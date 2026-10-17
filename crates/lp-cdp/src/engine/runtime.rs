//! `Runtime` domain over lp-js (upstream delegates this to v8_inspector).

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use lp_js::{ConsoleLevel, JsException, Persistent, Realm, RealmId, RootedVec, Scope, Value};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::remote::{self, Described};
use super::{EngineHost, Queued};
use crate::msg::{self, CdpError, CdpResult};
use crate::runtime_protocol::{CallArgument, ExecutionContextId, HandleTable, RemoteObject};
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
    group: Option<String>,
    pub(crate) context: ExecutionContextId,
    handles: Weak<RefCell<HandleTable<Persistent>>>,
    done: Cell<bool>,
}

impl Reply {
    fn value(&self, scope: &Scope<'_>, value: Value<'_>) {
        if self.done.get() {
            return;
        }
        let Some(handles) = self.handles.upgrade() else { return self.done.set(true) };
        let result = remote::describe(scope, value, self.by_value);
        if self.done.get() {
            return; // describe ran JavaScript, which may have cancelled us
        }
        match result.and_then(|d| remote::register(d, &handles, self.context, self.group.as_deref())) {
            Ok(remote) => self.send(&json!({ "result": remote })),
            Err(e) => self.error(&e),
        }
    }

    fn exception(&self, scope: &Scope<'_>, exception: &JsException, exception_id: i64) {
        if self.done.get() {
            return;
        }
        let details = remote::exception_details(scope, exception, exception_id, Some(self.context));
        let result = details.exception.clone().unwrap_or_else(RemoteObject::undefined);
        self.send(&json!({ "result": result, "exceptionDetails": details }));
    }

    fn send(&self, result: &serde_json::Value) {
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

/// Replies waiting for a promise to settle, held weakly (the promise
/// reactions own them).
#[derive(Default)]
pub(crate) struct PendingReplies {
    replies: RefCell<Vec<Weak<Reply>>>,
}

impl PendingReplies {
    fn register(&self, reply: &Rc<Reply>) -> CdpResult<()> {
        let mut replies = self.replies.borrow_mut();
        replies.retain(|r| r.upgrade().is_some_and(|r| !r.is_done()));
        if replies.len() >= MAX_PENDING_REPLIES {
            return Err(CdpError::server("Too many pending awaitPromise evaluations"));
        }
        replies.push(Rc::downgrade(reply));
        Ok(())
    }

    fn live(&self) -> Vec<Rc<Reply>> {
        self.replies.borrow().iter().filter_map(Weak::upgrade).filter(|r| !r.is_done()).collect()
    }

    /// Cancels every pending reply of a detached session.
    pub(crate) fn cancel_session(&self, session_id: &str) {
        for reply in self.live() {
            if reply.session_id == session_id {
                reply.cancel();
            }
        }
    }

    /// Fails every pending reply whose execution context was destroyed.
    pub(crate) fn fail_context(&self, context: ExecutionContextId) {
        for reply in self.live() {
            if reply.context == context {
                reply.error(&CdpError::server("Execution context was destroyed."));
            }
        }
    }
}

fn is_thenable(value: Value<'_>) -> bool {
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
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetPropertiesParams {
    object_id: String,
    #[serde(default)]
    own_properties: bool,
    #[serde(default)]
    accessor_properties_only: bool,
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
        let Some(p) = self.thread.browser.page(state.id) else { return };
        let frame_id = &p.main_frame.id.0;
        for r in &p.main_frame.realms {
            let origin = r.realm.origin().unwrap_or("").to_owned();
            let context = context_description(r.realm.id(), r.world.as_deref(), &origin, frame_id);
            self.event("Runtime.executionContextCreated", &json!({ "context": context }), session_id);
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
        group: Option<String>,
        realm: &Realm,
    ) -> CdpResult<Reply> {
        let session = self.sessions.get(cmd.session_id).ok_or_else(|| CdpError::server("session is gone"))?;
        Ok(Reply {
            collector: self.collector.clone(),
            session_id: cmd.session_id.to_owned(),
            id: cmd.command.id,
            by_value,
            group,
            context: context_id(realm.id()),
            handles: Rc::downgrade(&session.handles),
            done: Cell::new(false),
        })
    }

    fn evaluate(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<()> {
        let p: EvaluateParams = cmd.command.params()?;
        let realm = self.target_realm(cmd.page, p.context_id)?;
        let reply = self.reply_for(cmd, p.return_by_value, p.object_group, &realm)?;
        let exception_id = self.next_exception_id();
        let scope = realm.scope().map_err(remote::js_to_cdp)?;
        match scope.eval(&p.expression, None, 1) {
            Ok(value) => reply.settle(&self.pending, &scope, value, p.await_promise, exception_id),
            Err(error) => reply.exception(&scope, &error.details(&scope), exception_id),
        }
        Ok(())
    }

    fn call_function_on(&mut self, cmd: &PageCommand<'_, '_>) -> CdpResult<()> {
        let p: CallFunctionOnParams = cmd.command.params()?;
        let handles =
            self.sessions.get(cmd.session_id).ok_or_else(|| CdpError::server("session is gone"))?.handles.clone();
        // The target object decides the realm (and, unless overridden, the
        // object group, which CDP says is inherited); otherwise executionContextId.
        let (realm, this, inherited_group) = match &p.object_id {
            Some(object_id) => {
                let table = handles.borrow();
                let (context, handle) = table.get(object_id)?;
                let this = handle.clone();
                let group = table.group_of(object_id).map(str::to_owned);
                drop(table);
                (self.target_realm(cmd.page, Some(context))?, Some(this), group)
            }
            None if p.execution_context_id.is_some() => {
                (self.target_realm(cmd.page, p.execution_context_id)?, None, None)
            }
            None => return Err(CdpError::invalid_params("Either objectId or executionContextId must be specified")),
        };
        let context = context_id(realm.id());
        let group = p.object_group.or(inherited_group);
        let reply = self.reply_for(cmd, p.return_by_value, group, &realm)?;
        let exception_id = self.next_exception_id();
        let scope = realm.scope().map_err(remote::js_to_cdp)?;
        // Arguments live on the heap while the function source is compiled
        // and called, so they must be GC roots (docs/ARCHITECTURE.md rule 1).
        let mut args = RootedVec::new();
        for arg in &p.arguments {
            let value = remote::argument(&scope, arg, &handles, context)?;
            args.push(&scope, value);
        }
        match scope.call_function_rooted(&p.function_declaration, this.as_ref(), &args) {
            Ok(value) => reply.settle(&self.pending, &scope, value, p.await_promise, exception_id),
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
            let (context, handle) = table.get(&p.object_id)?;
            (context, handle.clone(), table.group_of(&p.object_id).map(str::to_owned))
        };
        let realm = self.target_realm(cmd.page, Some(context))?;
        let scope = realm.scope().map_err(remote::js_to_cdp)?;
        let object = parent.get(&scope);
        let object = object.as_object().ok_or_else(|| CdpError::server("Value with given id is not an object"))?;
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
            let mut parts = Vec::new();
            for (key, slot) in [("value", Slot::Value), ("get", Slot::Get), ("set", Slot::Set)] {
                if present(key) {
                    let v = entry.get(key).map_err(remote::js_to_cdp)?;
                    if slot != Slot::Value && v.is_undefined() {
                        continue;
                    }
                    parts.push((remote::describe(&scope, v, false)?, slot));
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
                },
                parts,
            ));
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
                }
            }
            result.push(descriptor);
        }
        Ok(json!({ "result": result }))
    }

    fn next_exception_id(&mut self) -> i64 {
        let id = self.next_exception_id;
        self.next_exception_id += 1;
        id
    }

    pub(super) fn on_console(&mut self, realm_id: RealmId, level: ConsoleLevel, args: Vec<Persistent>, timestamp: f64) {
        let Some(page) = self.page_of_realm(realm_id) else { return };
        let sessions = self.sessions_of(page, |s| s.runtime_enabled);
        if sessions.is_empty() {
            return;
        }
        let Some(realm) = self.realm(page, realm_id) else { return };
        let Ok(scope) = realm.scope() else { return };
        let context = context_id(realm_id);
        for session_id in sessions {
            let Some(handles) = self.sessions.get(&session_id).map(|s| s.handles.clone()) else { continue };
            let mut remote_args = Vec::with_capacity(args.len());
            for arg in &args {
                let described = remote::describe(&scope, arg.get(&scope), false);
                let remote = described.and_then(|d| remote::register(d, &handles, context, None));
                remote_args.push(remote.unwrap_or_else(|_| RemoteObject::undefined()));
            }
            self.event(
                "Runtime.consoleAPICalled",
                &json!({
                    "type": level.cdp_type(),
                    "args": remote_args,
                    "executionContextId": context,
                    "timestamp": timestamp,
                }),
                &session_id,
            );
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
}
