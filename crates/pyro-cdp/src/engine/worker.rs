//! Worker targets: the `Runtime` domain of a dedicated worker's session.
//!
//! The target layer owns worker sessions (`target.rs`); this module runs their
//! commands. A worker's realm lives on the worker's thread, so each command is
//! posted there (`WorkerHandle::inspect`) and answered from there, straight into
//! the connection's outbox, followed by a wake of the browser thread that writes
//! it. Everything a session sees comes from that one thread, in order. The state
//! on the thread (`WorkerRuntime`) exists once a command has arrived; before
//! that the worker buffers console calls and exceptions, and the first
//! `Runtime.enable` replays them, as V8's inspector does.
//!
//! Network events and interception of a worker's requests are reported on its
//! owner page's session, so `Network.*` and friends are accepted and do nothing.
//! Not implemented on worker sessions: `Runtime.getProperties`, `addBinding`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use pyro_browser::Browser;
use pyro_js::{ConsoleLevel, EngineHooks, JsException, Persistent, Realm, RootedVec, Scope, Value};
use pyro_runtime::LoopHandle;
use pyro_webapi::{BufferedEvent, WorkerContext, WorkerEvent};
use serde::Deserialize;
use serde_json::json;

use super::remote::{self, Described};
use super::runtime::{console_stack, context_id, is_thenable};
use super::{epoch_ms, EngineHost};
use crate::msg::{self, CdpError, CdpResult, Command};
use crate::outbox::Outbox;
use crate::runtime_protocol::{CallArgument, ExecutionContextId, HandleBudget, HandleTable, RemoteObject};
use crate::target::{PageKey, WorkerChange, WorkerKey};
use crate::ws::OutFrame;

impl EngineHost {
    pub(super) fn on_worker_event(&mut self, page: Option<PageKey>, event: WorkerEvent) {
        enum Kind {
            Dedicated,
            Service,
        }
        let event_kind = if matches!(event, WorkerEvent::ServiceCreated(_)) { Kind::Service } else { Kind::Dedicated };
        match event {
            WorkerEvent::Created(handle) | WorkerEvent::ServiceCreated(handle) => {
                let Some(page) = page else { return };
                let service = matches!(event_kind, Kind::Service);
                let worker = handle.id();
                self.worker_changes.push(WorkerChange::Started { worker, page, url: handle.url().to_owned(), service });
                self.workers.insert(worker, handle);
            }
            WorkerEvent::Destroyed(worker) => {
                self.workers.remove(&worker);
                self.worker_changes.push(WorkerChange::Stopped { worker });
            }
        }
    }

    pub(super) fn worker_session_command(&mut self, worker: WorkerKey, session_id: &str, cmd: &Command<'_>) {
        let (domain, _) = cmd.domain_and_action().unwrap_or(("", ""));
        if matches!(domain, "Network" | "Fetch" | "Security" | "Log" | "Debugger" | "Profiler" | "Inspector") {
            self.out.push(msg::result_frame(cmd.id, &json!({}), Some(session_id)));
            return;
        }
        if domain != "Runtime" {
            self.out.push(msg::error_frame(Some(cmd.id), &CdpError::unsupported(&cmd.method), Some(session_id)));
            return;
        }
        let Some(handle) = self.workers.get(&worker) else {
            let error = CdpError::server("The worker is gone");
            self.out.push(msg::error_frame(Some(cmd.id), &error, Some(session_id)));
            return;
        };
        let call = Call {
            id: cmd.id,
            method: cmd.method.to_string(),
            session: session_id.to_owned(),
            params: cmd.raw_params().map_or_else(|| "{}".to_owned(), |p| p.get().to_owned()),
        };
        let (out, wake) = (self.out.clone(), self.thread.handle());
        handle.inspect(Box::new(move |context| WorkerRuntime::of(context, out, wake).command(call)));
    }

    pub(super) fn worker_session_gone(&mut self, worker: WorkerKey, session_id: &str) {
        let Some(handle) = self.workers.get(&worker) else { return };
        let session = session_id.to_owned();
        handle.inspect(Box::new(move |context| {
            if let Some(runtime) = WorkerRuntime::existing(context) {
                runtime.sessions.borrow_mut().remove(&session);
            }
        }));
    }
}

struct Call {
    id: u64,
    method: String,
    session: String,
    params: String,
}

struct WorkerSession {
    enabled: Cell<bool>,
    handles: Rc<remote::Handles>,
}

/// One worker's inspector, on its thread. Promise reactions hold it weakly: they
/// live in the VM, which a strong reference (holding the realm) would keep alive.
struct WorkerRuntime {
    out: Outbox,
    wake: LoopHandle<Browser>,
    realm: Realm,
    budget: HandleBudget,
    sessions: RefCell<HashMap<String, Rc<WorkerSession>>>,
    /// What happened before any client asked; replayed by the first `Runtime.enable`.
    buffered: RefCell<Vec<BufferedEvent>>,
    next_exception: Cell<i64>,
}

/// How a result is reported (`Runtime.evaluate` and `callFunctionOn` parameters).
struct ReplyOptions {
    id: u64,
    session_id: String,
    session: Rc<WorkerSession>,
    by_value: bool,
    preview: bool,
    group: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EvaluateParams {
    expression: String,
    context_id: Option<ExecutionContextId>,
    #[serde(default)]
    return_by_value: bool,
    #[serde(default)]
    await_promise: bool,
    object_group: Option<String>,
    #[serde(default)]
    generate_preview: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CallFunctionOnParams {
    function_declaration: String,
    object_id: Option<String>,
    execution_context_id: Option<ExecutionContextId>,
    #[serde(default)]
    arguments: Vec<CallArgument>,
    #[serde(default)]
    return_by_value: bool,
    #[serde(default)]
    await_promise: bool,
    object_group: Option<String>,
    #[serde(default)]
    generate_preview: bool,
}

impl WorkerRuntime {
    fn existing(context: &WorkerContext<'_>) -> Option<Rc<WorkerRuntime>> {
        context.events.inspector_state()?.downcast::<WorkerRuntime>().ok()
    }

    fn of(context: &WorkerContext<'_>, out: Outbox, wake: LoopHandle<Browser>) -> Rc<WorkerRuntime> {
        if let Some(runtime) = Self::existing(context) {
            return runtime;
        }
        let runtime = Rc::new(WorkerRuntime {
            out,
            wake,
            realm: context.realm.clone(),
            budget: HandleBudget::new(HandleBudget::DEFAULT_LIMIT),
            sessions: RefCell::new(HashMap::new()),
            buffered: RefCell::new(Vec::new()),
            next_exception: Cell::new(1),
        });
        let buffered = context.events.attach_inspector(runtime.clone(), runtime.clone());
        *runtime.buffered.borrow_mut() = buffered;
        runtime
    }

    /// Queues a frame and wakes the browser thread that writes it.
    fn send(&self, frame: OutFrame) {
        self.out.push(frame);
        let _ = self.wake.post(|_| {});
    }

    fn session(&self, id: &str) -> Rc<WorkerSession> {
        self.sessions
            .borrow_mut()
            .entry(id.to_owned())
            .or_insert_with(|| {
                Rc::new(WorkerSession {
                    enabled: Cell::new(false),
                    handles: Rc::new(RefCell::new(HandleTable::new(self.budget.clone()))),
                })
            })
            .clone()
    }

    fn enabled_sessions(&self) -> Vec<(String, Rc<WorkerSession>)> {
        self.sessions.borrow().iter().filter(|(_, s)| s.enabled.get()).map(|(k, s)| (k.clone(), s.clone())).collect()
    }

    fn context(&self) -> ExecutionContextId {
        context_id(self.realm.id())
    }

    fn command(self: &Rc<Self>, call: Call) {
        let (id, session_id) = (call.id, call.session.clone());
        match self.run(call) {
            Ok(Some(result)) => self.send(msg::result_frame(id, &result, Some(&session_id))),
            Ok(None) => {}
            Err(error) => self.send(msg::error_frame(Some(id), &error, Some(&session_id))),
        }
    }

    fn run(self: &Rc<Self>, call: Call) -> CdpResult<Option<serde_json::Value>> {
        let session = self.session(&call.session);
        let invalid = |e: serde_json::Error| CdpError::invalid_params(e.to_string());
        match call.method.as_str() {
            "Runtime.enable" => {
                session.enabled.set(true);
                let origin = self.realm.origin().filter(|o| o != "null").unwrap_or_default();
                let description = json!({
                    "id": self.context(), "origin": origin, "name": "", "uniqueId": self.realm.id().0.to_string(),
                });
                let params = json!({ "context": description });
                self.send(msg::event_frame("Runtime.executionContextCreated", &params, Some(&call.session)));
                for event in self.buffered.take() {
                    match event {
                        BufferedEvent::Console { level, args, timestamp, stack } => {
                            self.console(level, &args, timestamp, stack)
                        }
                        BufferedEvent::Exception { exception, timestamp } => self.exception(&exception, timestamp),
                    }
                }
                Ok(Some(json!({})))
            }
            "Runtime.disable" => {
                session.enabled.set(false);
                Ok(Some(json!({})))
            }
            "Runtime.setAsyncCallStackDepth" | "Runtime.discardConsoleEntries" | "Runtime.terminateExecution" => {
                Ok(Some(json!({})))
            }
            "Runtime.evaluate" => {
                let p: EvaluateParams = serde_json::from_str(&call.params).map_err(invalid)?;
                self.check_context(p.context_id)?;
                let scope = self.realm.scope().map_err(remote::js_to_cdp)?;
                let options = ReplyOptions {
                    id: call.id,
                    session_id: call.session,
                    session,
                    by_value: p.return_by_value,
                    preview: p.generate_preview,
                    group: p.object_group,
                };
                let evaluated = scope.eval(&p.expression, None, 1);
                self.finish(&scope, evaluated, p.await_promise, options);
                Ok(None)
            }
            "Runtime.callFunctionOn" => {
                let p: CallFunctionOnParams = serde_json::from_str(&call.params).map_err(invalid)?;
                let (this, inherited) = match &p.object_id {
                    Some(object_id) => {
                        let table = session.handles.borrow();
                        let found = table.lookup(object_id)?;
                        (Some(found.handle.clone()), found.group.map(|g| g.to_string()))
                    }
                    None if p.execution_context_id.is_some() => {
                        self.check_context(p.execution_context_id)?;
                        (None, None)
                    }
                    None => {
                        return Err(CdpError::invalid_params("Either objectId or executionContextId must be specified"))
                    }
                };
                let scope = self.realm.scope().map_err(remote::js_to_cdp)?;
                let context = self.context();
                let mut args = RootedVec::new();
                for arg in &p.arguments {
                    let value = remote::argument(&scope, arg, &session.handles, context)?;
                    args.push(&scope, value);
                }
                let options = ReplyOptions {
                    id: call.id,
                    session_id: call.session,
                    session,
                    by_value: p.return_by_value,
                    preview: p.generate_preview,
                    group: p.object_group.or(inherited),
                };
                let called = scope.call_function_rooted(&p.function_declaration, this.as_deref(), &args);
                self.finish(&scope, called, p.await_promise, options);
                Ok(None)
            }
            "Runtime.releaseObject" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    object_id: String,
                }
                let id = serde_json::from_str::<P>(&call.params).map_err(invalid)?.object_id;
                let released = session.handles.borrow_mut().release(&id);
                drop(released);
                Ok(Some(json!({})))
            }
            "Runtime.releaseObjectGroup" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    object_group: String,
                }
                let group = serde_json::from_str::<P>(&call.params).map_err(invalid)?.object_group;
                session.handles.borrow_mut().release_group(&group);
                Ok(Some(json!({})))
            }
            other => Err(CdpError::unsupported(other)),
        }
    }

    fn check_context(&self, context: Option<ExecutionContextId>) -> CdpResult<()> {
        match context {
            Some(id) if id != self.context() => Err(CdpError::server("Cannot find context with specified id")),
            _ => Ok(()),
        }
    }

    fn finish(
        self: &Rc<Self>,
        scope: &Scope<'_>,
        result: pyro_js::JsResult<Value<'_>>,
        await_promise: bool,
        options: ReplyOptions,
    ) {
        match result {
            Ok(value) if await_promise && is_thenable(value) => {
                let options = Rc::new(options);
                let (ok, err) = (options.clone(), options.clone());
                let (this, other) = (Rc::downgrade(self), Rc::downgrade(self));
                let then = scope.then(
                    value,
                    move |s, v| {
                        if let Some(runtime) = Weak::upgrade(&this) {
                            runtime.answer(s, v, &ok);
                        }
                    },
                    move |s, v| {
                        if let Some(runtime) = Weak::upgrade(&other) {
                            let exception = JsException::from_value(s, v);
                            runtime.answer_exception(s, &exception, &err);
                        }
                    },
                );
                if let Err(e) = then {
                    let frame = msg::error_frame(Some(options.id), &remote::js_to_cdp(e), Some(&options.session_id));
                    self.send(frame);
                }
            }
            Ok(value) => self.answer(scope, value, &options),
            Err(pyro_js::JsError::Terminated) => {
                let error = CdpError::server("Execution was terminated");
                self.send(msg::error_frame(Some(options.id), &error, Some(&options.session_id)));
            }
            Err(error) => self.answer_exception(scope, &error.details(scope), &options),
        }
    }

    fn answer(&self, scope: &Scope<'_>, value: Value<'_>, options: &ReplyOptions) {
        let result = remote::describe(scope, value, options.by_value)
            .map(|mut d| {
                if options.preview && d.keep.is_some() {
                    remote::add_preview(scope, value, &mut d.remote);
                }
                d
            })
            .and_then(|d| remote::register(d, &options.session.handles, self.context(), options.group.as_deref()));
        let frame = match result {
            Ok(remote) => msg::result_frame(options.id, &json!({ "result": remote }), Some(&options.session_id)),
            Err(error) => msg::error_frame(Some(options.id), &error, Some(&options.session_id)),
        };
        self.send(frame);
    }

    fn answer_exception(&self, scope: &Scope<'_>, exception: &JsException, options: &ReplyOptions) {
        let details = remote::exception_details(scope, exception, self.exception_id(), Some(self.context()));
        let result = details.exception.clone().unwrap_or_else(RemoteObject::undefined);
        let body = json!({ "result": result, "exceptionDetails": details });
        self.send(msg::result_frame(options.id, &body, Some(&options.session_id)));
    }

    fn exception_id(&self) -> i64 {
        let id = self.next_exception.get();
        self.next_exception.set(id + 1);
        id
    }

    fn console(&self, level: ConsoleLevel, args: &[Persistent], timestamp: f64, stack: Option<String>) {
        let Ok(scope) = self.realm.scope() else { return };
        let context = self.context();
        let stack_trace = stack.map(|s| console_stack(&s));
        for (session_id, session) in self.enabled_sessions() {
            let remote_args: Vec<RemoteObject> = args
                .iter()
                .map(|arg| {
                    let value = arg.get(&scope);
                    remote::describe(&scope, value, false)
                        .map(|mut d: Described| {
                            if d.keep.is_some() {
                                remote::add_preview(&scope, value, &mut d.remote);
                            }
                            d
                        })
                        .and_then(|d| remote::register(d, &session.handles, context, None))
                        .unwrap_or_else(|_| RemoteObject::undefined())
                })
                .collect();
            let mut params = json!({
                "type": level.cdp_type(),
                "args": remote_args,
                "executionContextId": context,
                "timestamp": timestamp,
            });
            if let Some(trace) = stack_trace.as_ref() {
                params["stackTrace"] = json!(trace);
            }
            self.send(msg::event_frame("Runtime.consoleAPICalled", &params, Some(&session_id)));
        }
    }

    fn exception(&self, exception: &JsException, timestamp: f64) {
        let Ok(scope) = self.realm.scope() else { return };
        let sessions = self.enabled_sessions();
        if sessions.is_empty() {
            return;
        }
        let details = remote::exception_details(&scope, exception, self.exception_id(), Some(self.context()));
        let params = json!({ "timestamp": timestamp, "exceptionDetails": details });
        for (session_id, _) in sessions {
            self.send(msg::event_frame("Runtime.exceptionThrown", &params, Some(&session_id)));
        }
    }
}

impl EngineHooks for WorkerRuntime {
    fn console_api(&self, realm: &Realm, level: ConsoleLevel, args: &[Value<'_>]) {
        let Ok(scope) = realm.scope() else { return };
        let args: Vec<Persistent> = args.iter().map(|v| Persistent::new(&scope, *v)).collect();
        self.console(level, &args, epoch_ms(), scope.current_stack().ok());
    }

    fn exception_thrown(&self, _realm: &Realm, exception: &JsException) {
        self.exception(exception, epoch_ms());
    }

    fn unhandled_rejection(&self, _realm: &Realm, exception: &JsException) {
        self.exception(exception, epoch_ms());
    }
}
