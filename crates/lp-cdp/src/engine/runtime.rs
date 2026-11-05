//! `Runtime` domain over lp-js (upstream delegates this to v8_inspector).

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use lp_js::{ConsoleLevel, JsException, Persistent, Realm, RealmId, RootedVec, Scope, Value};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::remote::{self, Described};
use super::{EngineHost, Queued};
use crate::msg::{self, CdpError, CdpResult};
use crate::runtime_protocol::{CallArgument, ExceptionDetails, ExecutionContextId, RemoteObject};
use crate::target::{PageCommand, PageKey};

pub(crate) fn context_id(realm: RealmId) -> ExecutionContextId {
    i64::from(realm.0)
}

/// `Runtime.ExecutionContextDescription` for a realm of `frame_id`.
pub(crate) fn context_description(
    realm: RealmId,
    world: Option<&str>,
    origin: &str,
    frame_id: &str,
) -> serde_json::Value {
    let origin = if origin == "null" { "" } else { origin };
    json!({
        "id": context_id(realm),
        "origin": origin,
        "name": world.unwrap_or(""),
        "uniqueId": format!("{}", realm.0),
        "auxData": {
            "isDefault": world.is_none(),
            "type": if world.is_none() { "default" } else { "isolated" },
            "frameId": frame_id,
        },
    })
}

/// Most `awaitPromise` replies that may wait for settlement per connection.
pub(crate) const MAX_PENDING_REPLIES: usize = 10_000;

/// What to do with a value once it is available (now or after a promise settles).
///
/// A reply is answered at most once. Pending replies are registered with the
/// host: detaching their session cancels them silently, and destroying their
/// execution context answers them with an error, as Chrome does. The handle
/// table is held weakly so a closure kept alive by JavaScript cannot keep a
/// detached session's handles.
pub(crate) struct Reply {
    /// Replies go through the engine notification queue, not straight to the
    /// connection, so they stay behind events the evaluation produced.
    collector: Rc<super::Collector>,
    pub(crate) session_id: String,
    id: u64,
    by_value: bool,
    /// `generatePreview`: object results carry a `Runtime.ObjectPreview`.
    preview: bool,
    group: Option<Rc<str>>,
    pub(crate) context: ExecutionContextId,
    handles: Weak<remote::Handles>,
    done: Cell<bool>,
}

/// `{"result": .., "exceptionDetails": ..}` serialized straight into the
/// frame: going through `json!` would copy a by-value result into a tree.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ResultBody<'a> {
    result: &'a RemoteObject,
    #[serde(skip_serializing_if = "Option::is_none")]
    exception_details: Option<&'a ExceptionDetails>,
}

impl Reply {
    fn value(&self, scope: &Scope<'_>, value: Value<'_>) {
        if self.done.get() {
            return;
        }
        let Some(handles) = self.handles.upgrade() else { return self.done.set(true) };
        let describe = crate::perf::span("remote.describe");
        let result = remote::describe(scope, value, self.by_value).map(|mut d| {
            if self.preview && d.keep.is_some() {
                remote::add_preview(scope, value, &mut d.remote);
            }
            d
        });
        drop(describe);
        if self.done.get() {
            return; // describe ran JavaScript, which may have cancelled us
        }
        let _t = crate::perf::span("remote.register+reply");
        match result.and_then(|d| remote::register(d, &handles, self.context, self.group.as_deref())) {
            Ok(remote) => self.send(&ResultBody { result: &remote, exception_details: None }),
            Err(e) => self.error(&e),
        }
    }

    fn exception(&self, scope: &Scope<'_>, exception: &JsException, exception_id: i64) {
        if self.done.get() {
            return;
        }
        let details = remote::exception_details(scope, exception, exception_id, Some(self.context));
        let result = details.exception.clone().unwrap_or_else(RemoteObject::undefined);
        self.send(&ResultBody { result: &result, exception_details: Some(&details) });
    }

    fn send<T: serde::Serialize>(&self, result: &T) {
        if !self.done.replace(true) {
            self.collector.push(Queued::Frame(msg::result_frame(self.id, result, Some(&self.session_id))));
        }
    }

    pub(crate) fn error(&self, error: &CdpError) {
        if !self.done.replace(true) {
            self.collector.push(Queued::Frame(msg::error_frame(Some(self.id), error, Some(&self.session_id))));
        }
    }

    /// Drops the reply without answering (its session is gone).
    pub(crate) fn cancel(&self) {
        self.done.set(true);
    }

    pub(crate) fn is_done(&self) -> bool {
        self.done.get()
    }

    /// Replies with `value`, or with its settlement when `await_promise` is
    /// set and the value is thenable. The awaited reply runs as a microtask.
    fn settle(
        self,
        host_pending: &PendingReplies,
        scope: &Scope<'_>,
        value: Value<'_>,
        await_promise: bool,
        exception_id: i64,
    ) {
        if !await_promise || !is_thenable(value) {
            self.value(scope, value);
            return;
        }
        let reply = Rc::new(self);
        if let Err(e) = host_pending.register(&reply) {
            reply.error(&e);
            return;
        }
        let (ok, err) = (reply.clone(), reply.clone());
        let then = scope.then(
            value,
            move |s, v| ok.value(s, v),
            move |s, v| {
                let exception = JsException::from_value(s, v);
                err.exception(s, &exception, exception_id);
            },
        );
        if let Err(e) = then {
            reply.error(&remote::js_to_cdp(e));
        }
    }
}

/// Replies waiting for a promise to settle, held strongly.
///
/// The promise reactions hold the reply too, but a never-settling promise
/// nobody references is garbage: once collected (release builds do so before
/// a later navigation), a weakly held reply was gone and `fail_context` had
/// nothing left to answer, so the evaluation never got its reply. A `Reply`
/// owns only the collector queue, strings and a weak handle table, so holding
/// it here creates no cycle; answered or cancelled entries are pruned on every
/// table operation and the table is bounded by `limit`.
pub(crate) struct PendingReplies {
    replies: RefCell<Vec<Rc<Reply>>>,
    limit: Cell<usize>,
}

impl Default for PendingReplies {
    fn default() -> Self {
        PendingReplies { replies: RefCell::new(Vec::new()), limit: Cell::new(MAX_PENDING_REPLIES) }
    }
}

impl PendingReplies {
    fn prune(&self) {
        self.replies.borrow_mut().retain(|r| !r.is_done());
    }

    fn register(&self, reply: &Rc<Reply>) -> CdpResult<()> {
        self.prune();
        let mut replies = self.replies.borrow_mut();
        if replies.len() >= self.limit.get() {
            return Err(CdpError::server("Too many pending awaitPromise evaluations"));
        }
        replies.push(reply.clone());
        Ok(())
    }

    /// The unanswered replies (a snapshot: answering one must not re-enter the table).
    fn live(&self) -> Vec<Rc<Reply>> {
        self.prune();
        self.replies.borrow().clone()
    }

    /// Cancels every pending reply of a detached session.
    pub(crate) fn cancel_session(&self, session_id: &str) {
        for reply in self.live() {
            if reply.session_id == session_id {
                reply.cancel();
            }
        }
        self.prune();
    }

    /// Fails every pending reply whose execution context was destroyed.
    pub(crate) fn fail_context(&self, context: ExecutionContextId) {
        for reply in self.live() {
            if reply.context == context {
                reply.error(&CdpError::server("Execution context was destroyed."));
            }
        }
        self.prune();
    }

    /// Cancels everything (connection teardown).
    pub(crate) fn cancel_all(&self) {
        for reply in self.live() {
            reply.cancel();
        }
        self.prune();
    }

    /// Unanswered replies currently held.
    pub(crate) fn len(&self) -> usize {
        self.prune();
        self.replies.borrow().len()
    }

    pub(crate) fn set_limit(&self, limit: usize) {
        self.limit.set(limit);
    }
}

pub(super) fn is_thenable(value: Value<'_>) -> bool {
    value.as_object().is_some_and(|o| o.get("then").is_ok_and(|t| t.is_function()))
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
struct CallFunctionOnParams<'a> {
    /// Borrowed from the message: Playwright sends a ~40 KB declaration per call.
    #[serde(borrow)]
    function_declaration: std::borrow::Cow<'a, str>,
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

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetPropertiesParams {
    object_id: String,
    #[serde(default)]
    own_properties: bool,
    #[serde(default)]
    accessor_properties_only: bool,
    #[serde(default)]
    non_indexed_properties_only: bool,
    #[serde(default)]
    generate_preview: bool,
}

#[derive(Serialize)]
struct InternalPropertyDescriptor {
    name: String,
    value: RemoteObject,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PropertyDescriptor {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<RemoteObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    writable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    get: Option<RemoteObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    set: Option<RemoteObject>,
    configurable: bool,
    enumerable: bool,
    is_own: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol: Option<RemoteObject>,
}

impl EngineHost {
    pub(super) fn runtime_domain(
        &mut self,
        action: &str,
        cmd: &PageCommand<'_, '_>,
    ) -> CdpResult<Option<serde_json::Value>> {
        match action {
            "enable" => {
                self.session_mut(cmd.session_id)?.runtime_enabled = true;
                self.update_runtime_wanted();
                self.announce_contexts(cmd.page, cmd.session_id);
                Ok(Some(json!({})))
            }
            "disable" => {
                self.session_mut(cmd.session_id)?.runtime_enabled = false;
                self.update_runtime_wanted();
                Ok(Some(json!({})))
            }
            // Defined in the page's matching realms now and in every new one
            // (before init scripts); calls come back as Runtime.bindingCalled.
            "addBinding" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    name: String,
                    execution_context_id: Option<ExecutionContextId>,
                    execution_context_name: Option<String>,
                }
                let p: P = cmd.command.params()?;
                if p.name.is_empty() {
                    return Err(CdpError::invalid_params("binding name must not be empty"));
                }
                if p.execution_context_id.is_some() && p.execution_context_name.is_some() {
                    return Err(CdpError::invalid_params(
                        "executionContextName is mutually exclusive with executionContextId",
                    ));
                }
                if let Some(id) = p.execution_context_id {
                    // Deprecated in Chrome: that context only, not kept.
                    let realm = self.target_realm(cmd.page, Some(id))?;
                    realm.scope().and_then(|s| s.install_binding(&p.name)).map_err(remote::js_to_cdp)?;
                } else {
                    let pid = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
                    let binding = lp_browser::Binding { name: p.name.clone(), world: p.execution_context_name };
                    self.thread.browser.add_binding(pid, binding);
                }
                let session = self.session_mut(cmd.session_id)?;
                if !session.bindings.contains(&p.name) {
                    session.bindings.push(p.name);
                }
                Ok(Some(json!({})))
            }
            "removeBinding" => {
                #[derive(Deserialize)]
                struct P {
                    name: String,
                }
                let name = cmd.command.params::<P>()?.name;
                self.session_mut(cmd.session_id)?.bindings.retain(|b| *b != name);
                // Other sessions of the page may still use it.
                if self.sessions_of(cmd.page, |s| s.bindings.contains(&name)).is_empty() {
                    let pid = self.pages.get(&cmd.page).ok_or_else(|| CdpError::server("page is gone"))?.id;
                    self.thread.browser.remove_binding(pid, &name);
                }
                Ok(Some(json!({})))
            }
            // No async stacks are recorded (console calls carry their
            // synchronous stack regardless).
            "setAsyncCallStackDepth" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                #[allow(dead_code)]
                struct P {
                    max_depth: i64,
                }
                cmd.command.params::<P>()?;
                Ok(Some(json!({})))
            }
            // A script running when it arrives was already stopped by the
            // connection's watchdog check (conn.rs `check_during_script`),
            // its evaluation answered "Execution was terminated".
            "terminateExecution" => Ok(Some(json!({}))),
            // JavaScriptCore's public API has no heap iteration (V8's
            // inspector walks the heap for this).
            "queryObjects" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    prototype_object_id: String,
                }
                let id = cmd.command.params::<P>()?.prototype_object_id;
                let handles = self.sessions.get(cmd.session_id).ok_or_else(|| CdpError::server("session is gone"))?;
                let (context, handle) = {
                    let table = handles.handles.borrow();
                    let (context, handle) =
                        table.get(&id).map_err(|_| CdpError::server("Could not find object with given id"))?;
                    (context, handle.clone())
                };
                let realm = self.target_realm(cmd.page, Some(context))?;
                let scope = realm.scope().map_err(remote::js_to_cdp)?;
                if !handle.get(&scope).is_object() {
                    return Err(CdpError::server("Prototype should be instance of Object"));
                }
                Err(CdpError::server("Runtime.queryObjects is not supported: JavaScriptCore exposes no heap iteration"))
            }
            "evaluate" => self.evaluate(cmd).map(|()| None),
            "callFunctionOn" => self.call_function_on(cmd).map(|()| None),
            "getProperties" => self.get_properties(cmd).map(Some),
            "releaseObject" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    object_id: String,
                }
                let id = cmd.command.params::<P>()?.object_id;
                // Drop the handle outside the borrow: releasing may run GC work.
                let released = self.session_mut(cmd.session_id)?.handles.borrow_mut().release(&id);
                drop(released);
                Ok(Some(json!({})))
            }
            "releaseObjectGroup" => {
                #[derive(Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct P {
                    object_group: String,
                }
                let group = cmd.command.params::<P>()?.object_group;
                self.session_mut(cmd.session_id)?.handles.borrow_mut().release_group(&group);
                Ok(Some(json!({})))
            }
            _ => Err(CdpError::unsupported(&cmd.command.method)),
        }
    }

    pub(crate) fn session_mut(&mut self, session_id: &str) -> CdpResult<&mut super::Session> {
        self.sessions.get_mut(session_id).ok_or_else(|| CdpError::new(-32001, "Session with given id not found."))
    }

    /// `Runtime.executionContextCreated` for every live realm of the page.
    pub(super) fn announce_contexts(&self, page: PageKey, session_id: &str) {
        let Some(state) = self.pages.get(&page) else { return };
        for info in self.thread.browser.frames(state.id) {
            let Some(frame) = self.thread.browser.frame(state.id, &info.id) else { continue };
            for r in &frame.realms {
                let origin = r.realm.origin().unwrap_or_default();
                let context = context_description(r.realm.id(), r.world.as_deref(), &origin, &info.id.0);
                self.event("Runtime.executionContextCreated", &json!({ "context": context }), session_id);
            }
        }
    }

    pub(super) fn target_realm(&self, page: PageKey, context: Option<ExecutionContextId>) -> CdpResult<Realm> {
        let not_found = || CdpError::server("Cannot find context with specified id");
        match context {
            Some(id) => {
                let realm = u32::try_from(id).map(RealmId).map_err(|_| not_found())?;
                self.realm(page, realm).ok_or_else(not_found)
            }
            None => self.main_realm(page).ok_or_else(not_found),
        }
    }

    fn reply_for(
        &self,
        cmd: &PageCommand<'_, '_>,
        by_value: bool,
        preview: bool,
        group: Option<Rc<str>>,
        realm: &Realm,
    ) -> CdpResult<Reply> {
        let session = self.sessions.get(cmd.session_id).ok_or_else(|| CdpError::server("session is gone"))?;
        Ok(Reply {
            collector: self.collector.clone(),
            session_id: cmd.session_id.to_owned(),
            id: cmd.command.id,
            by_value,
            preview,
            group,
            context: context_id(realm.id()),
            handles: Rc::downgrade(&session.handles),
            done: Cell::new(false),
        })
    }

    fn evaluate(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<()> {
        let p: EvaluateParams = cmd.command.params()?;
        let realm = self.target_realm(cmd.page, p.context_id)?;
        let group = p.object_group.map(Rc::from);
        let reply = self.reply_for(cmd, p.return_by_value, p.generate_preview, group, &realm)?;
        let exception_id = self.next_exception_id();
        let scope = realm.scope().map_err(remote::js_to_cdp)?;
        let eval = crate::perf::span("js.eval");
        let evaluated = scope.eval(&p.expression, None, 1);
        drop(eval);
        match evaluated {
            Ok(value) => reply.settle(&self.pending, &scope, value, p.await_promise, exception_id),
            Err(lp_js::JsError::Terminated) => reply.error(&terminated()),
            Err(error) => reply.exception(&scope, &error.details(&scope), exception_id),
        }
        Ok(())
    }

    fn call_function_on(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<()> {
        let p: CallFunctionOnParams<'_> = cmd.command.params_borrowed()?;
        let handles =
            self.sessions.get(cmd.session_id).ok_or_else(|| CdpError::server("session is gone"))?.handles.clone();
        // The target object decides the realm (and, unless overridden, the
        // object group, which CDP says is inherited); otherwise executionContextId.
        let (realm, this, inherited_group) = match &p.object_id {
            Some(object_id) => {
                let table = handles.borrow();
                let found = table.lookup(object_id)?;
                let (context, this, group) = (found.context, found.handle.clone(), found.group.cloned());
                drop(table);
                (self.target_realm(cmd.page, Some(context))?, Some(this), group)
            }
            None if p.execution_context_id.is_some() => {
                (self.target_realm(cmd.page, p.execution_context_id)?, None, None)
            }
            None => return Err(CdpError::invalid_params("Either objectId or executionContextId must be specified")),
        };
        let context = context_id(realm.id());
        let group = p.object_group.map(Rc::from).or(inherited_group);
        let reply = self.reply_for(cmd, p.return_by_value, p.generate_preview, group, &realm)?;
        let exception_id = self.next_exception_id();
        let scope = realm.scope().map_err(remote::js_to_cdp)?;
        // Arguments live on the heap while the function source is compiled
        // and called, so they must be GC roots (docs/ARCHITECTURE.md rule 1).
        let mut args = RootedVec::new();
        for arg in &p.arguments {
            let value = remote::argument(&scope, arg, &handles, context)?;
            args.push(&scope, value);
        }
        let call = crate::perf::span("js.call_function");
        let called = scope.call_function_rooted(&p.function_declaration, this.as_deref(), &args);
        drop(call);
        match called {
            Ok(value) => reply.settle(&self.pending, &scope, value, p.await_promise, exception_id),
            Err(lp_js::JsError::Terminated) => reply.error(&terminated()),
            Err(error) => reply.exception(&scope, &error.details(&scope), exception_id),
        }
        Ok(())
    }

    fn get_properties(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<serde_json::Value> {
        let p: GetPropertiesParams = cmd.command.params()?;
        let handles =
            self.sessions.get(cmd.session_id).ok_or_else(|| CdpError::server("session is gone"))?.handles.clone();
        let (context, parent, group) = {
            let table = handles.borrow();
            let found = table.lookup(&p.object_id)?;
            (found.context, found.handle.clone(), found.group.cloned())
        };
        let realm = self.target_realm(cmd.page, Some(context))?;
        let scope = realm.scope().map_err(remote::js_to_cdp)?;
        let object = parent.get(&scope);
        let object = object.as_object().ok_or_else(|| CdpError::server("Value with given id is not an object"))?;
        let describe = |v: Value<'_>| -> CdpResult<Described> {
            let mut d = remote::describe(&scope, v, false)?;
            if p.generate_preview && d.keep.is_some() {
                remote::add_preview(&scope, v, &mut d.remote);
            }
            Ok(d)
        };
        let list = scope.own_properties(object, p.own_properties).map_err(remote::js_to_cdp)?;
        let len = list.get("length").and_then(|l| l.to_number()).map_err(remote::js_to_cdp)? as u32;
        let mut collected: Vec<(PropertyDescriptor, Vec<(Described, Slot)>)> = Vec::with_capacity(len as usize);
        for i in 0..len {
            let entry = list.get_index(i).map_err(remote::js_to_cdp)?;
            let entry = entry.as_object().ok_or_else(|| CdpError::server("malformed property descriptor"))?;
            let flag = |k: &str| entry.get(k).map(|v| v.to_bool()).unwrap_or(false);
            let present = |k: &str| entry.has(k);
            let is_accessor = present("get") || present("set");
            if p.accessor_properties_only && !is_accessor {
                continue;
            }
            let name = entry.get("name").ok().and_then(|n| n.as_string()).unwrap_or_default();
            if p.non_indexed_properties_only
                && !present("symbol")
                && name.parse::<u32>().is_ok_and(|i| i.to_string() == name)
            {
                continue;
            }
            let mut parts = Vec::new();
            for (key, slot) in
                [("value", Slot::Value), ("get", Slot::Get), ("set", Slot::Set), ("symbol", Slot::Symbol)]
            {
                if present(key) {
                    let v = entry.get(key).map_err(remote::js_to_cdp)?;
                    parts.push((describe(v)?, slot));
                } else if is_accessor && matches!(slot, Slot::Get | Slot::Set) {
                    // Chrome reports the missing half of an accessor as undefined.
                    parts.push((Described { remote: RemoteObject::undefined(), keep: None }, slot));
                }
            }
            collected.push((
                PropertyDescriptor {
                    name,
                    value: None,
                    writable: (!is_accessor).then(|| flag("writable")),
                    get: None,
                    set: None,
                    configurable: flag("configurable"),
                    enumerable: flag("enumerable"),
                    is_own: flag("isOwn"),
                    symbol: None,
                },
                parts,
            ));
        }
        // Internal slots ([[Prototype]], [[PrimitiveValue]], [[Entries]],
        // a promise's state); not with accessorPropertiesOnly, as Chrome.
        let mut internal: Vec<(String, Described)> = Vec::new();
        if !p.accessor_properties_only {
            let pairs = scope.internal_properties(object, true).map_err(remote::js_to_cdp)?;
            let n = pairs.get("length").and_then(|l| l.to_number()).map_err(remote::js_to_cdp)? as u32;
            for i in 0..n {
                let Some(pair) = pairs.get_index(i).ok().and_then(|v| v.as_object()) else { continue };
                let name = pair.get_index(0).ok().and_then(|n| n.as_string()).unwrap_or_default();
                let value = pair.get_index(1).map_err(remote::js_to_cdp)?;
                internal.push((name, describe(value)?));
            }
            if let Some((state, result)) = remote::promise_state(&scope, object.value()) {
                internal.push(("[[PromiseState]]".to_owned(), describe(scope.string(&state))?));
                if let Some(result) = result {
                    internal.push(("[[PromiseResult]]".to_owned(), describe(result.get(&scope))?));
                }
            }
        }
        let mut result = Vec::with_capacity(collected.len());
        for (mut descriptor, parts) in collected {
            for (described, slot) in parts {
                // Property handles inherit the object's group (CDP semantics).
                let remote = remote::register(described, &handles, context, group.as_deref())?;
                match slot {
                    Slot::Value => descriptor.value = Some(remote),
                    Slot::Get => descriptor.get = Some(remote),
                    Slot::Set => descriptor.set = Some(remote),
                    Slot::Symbol => descriptor.symbol = Some(remote),
                }
            }
            result.push(descriptor);
        }
        if internal.is_empty() {
            return Ok(json!({ "result": result }));
        }
        let mut internal_properties = Vec::with_capacity(internal.len());
        for (name, described) in internal {
            let value = remote::register(described, &handles, context, group.as_deref())?;
            internal_properties.push(InternalPropertyDescriptor { name, value });
        }
        Ok(json!({ "result": result, "internalProperties": internal_properties }))
    }

    fn next_exception_id(&mut self) -> i64 {
        let id = self.next_exception_id;
        self.next_exception_id += 1;
        id
    }

    /// `Runtime.bindingCalled` to the page's sessions that added the binding.
    pub(super) fn on_binding_called(&mut self, realm_id: RealmId, name: String, payload: String) {
        let Some(page) = self.page_of_realm(realm_id) else { return };
        let params = json!({ "name": name, "payload": payload, "executionContextId": context_id(realm_id) });
        for session_id in self.sessions_of(page, |s| s.bindings.contains(&name)) {
            self.event("Runtime.bindingCalled", &params, &session_id);
        }
    }

    pub(super) fn on_console(
        &mut self,
        realm_id: RealmId,
        level: ConsoleLevel,
        args: Vec<Persistent>,
        timestamp: f64,
        stack: Option<String>,
    ) {
        let Some(page) = self.page_of_realm(realm_id) else { return };
        let sessions = self.sessions_of(page, |s| s.runtime_enabled);
        if sessions.is_empty() {
            return;
        }
        let Some(realm) = self.realm(page, realm_id) else { return };
        let Ok(scope) = realm.scope() else { return };
        let context = context_id(realm_id);
        // Described once for every session; objects carry a preview, as in
        // Chrome (Playwright renders console text for objects from it).
        let described: Vec<Option<(RemoteObject, Option<Rc<Persistent>>)>> = args
            .iter()
            .map(|arg| {
                let value = arg.get(&scope);
                let mut d = remote::describe(&scope, value, false).ok()?;
                if d.keep.is_some() {
                    remote::add_preview(&scope, value, &mut d.remote);
                }
                Some((d.remote, d.keep.map(Rc::new)))
            })
            .collect();
        // Parsed once for every session.
        let stack_trace = stack.map(|s| console_stack(&s));
        for session_id in sessions {
            let Some(session) = self.sessions.get(&session_id) else { continue };
            let handles = session.handles.clone();
            let mut remote_args = Vec::with_capacity(args.len());
            for d in &described {
                let remote = d
                    .clone()
                    .ok_or_else(|| CdpError::server("unconvertible"))
                    .and_then(|(remote, keep)| remote::register_shared(remote, keep, &handles, context, None));
                remote_args.push(remote.unwrap_or_else(|_| RemoteObject::undefined()));
            }
            let mut params = json!({
                "type": level.cdp_type(),
                "args": remote_args,
                "executionContextId": context,
                "timestamp": timestamp,
            });
            if let Some(trace) = stack_trace.as_ref() {
                params["stackTrace"] = json!(trace);
            }
            self.event("Runtime.consoleAPICalled", &params, &session_id);
        }
    }

    pub(super) fn on_exception(&mut self, realm_id: RealmId, exception: JsException, timestamp: f64) {
        let Some(page) = self.page_of_realm(realm_id) else { return };
        let sessions = self.sessions_of(page, |s| s.runtime_enabled);
        if sessions.is_empty() {
            return;
        }
        let Some(realm) = self.realm(page, realm_id) else { return };
        let Ok(scope) = realm.scope() else { return };
        let exception_id = self.next_exception_id();
        let details = remote::exception_details(&scope, &exception, exception_id, Some(context_id(realm_id)));
        for session_id in sessions {
            self.event(
                "Runtime.exceptionThrown",
                &json!({ "timestamp": timestamp, "exceptionDetails": details }),
                &session_id,
            );
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    Value,
    Get,
    Set,
    Symbol,
}

fn terminated() -> CdpError {
    CdpError::server("Execution was terminated")
}

/// `Runtime.StackTrace` of a console call from the stack captured inside
/// it (helpers.js `stack`): the frames above the console method's own
/// native frame (the capturing helper's and the method's are dropped).
pub(super) fn console_stack(stack: &str) -> crate::runtime_protocol::StackTrace {
    // The helper's frame, then the console method's native one; matched by
    // location, never by function name.
    let lines: Vec<&str> = stack.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.iter().position(|l| l.ends_with("@[native code]")).map_or(1, |i| i + 1);
    let rest = lines[start.min(lines.len())..].join("\n");
    crate::runtime_protocol::StackTrace { call_frames: crate::runtime_protocol::parse_jsc_stack(&rest) }
}
