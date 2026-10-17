//! Conversion between engine values and CDP `Runtime.RemoteObject`s.
//!
//! Conversion never holds a borrow of the handle table while JavaScript runs:
//! [`describe`] calls into the engine and returns the value to keep, and the
//! caller inserts it afterwards. Running JavaScript can drain microtasks,
//! which may run other pending CDP replies that use the same table.

use std::cell::RefCell;

use lp_js::{JsError, JsException, JsType, Persistent, Scope, Value};
use serde_json::Value as Json;

use crate::msg::{CdpError, CdpResult};
use crate::runtime_protocol::{
    parse_jsc_stack, v8_style_description, Argument, CallArgument, ExceptionDetails, ExecutionContextId, HandleTable,
    RemoteObject, StackTrace,
};

/// A converted value plus the engine reference to store if the client should
/// get an `objectId`.
pub(crate) struct Described {
    pub(crate) remote: RemoteObject,
    pub(crate) keep: Option<Persistent>,
}

fn static_str(s: &str) -> &'static str {
    // RemoteObject.type/subtype are drawn from fixed CDP vocabularies.
    match s {
        "object" => "object",
        "function" => "function",
        "undefined" => "undefined",
        "string" => "string",
        "number" => "number",
        "boolean" => "boolean",
        "symbol" => "symbol",
        "bigint" => "bigint",
        "array" => "array",
        "null" => "null",
        "node" => "node",
        "regexp" => "regexp",
        "date" => "date",
        "map" => "map",
        "set" => "set",
        "weakmap" => "weakmap",
        "weakset" => "weakset",
        "iterator" => "iterator",
        "generator" => "generator",
        "error" => "error",
        "proxy" => "proxy",
        "promise" => "promise",
        "typedarray" => "typedarray",
        "arraybuffer" => "arraybuffer",
        "dataview" => "dataview",
        "webassemblymemory" => "webassemblymemory",
        "wasmvalue" => "wasmvalue",
        _ => "object",
    }
}

/// Describes `value`. `by_value` requests a JSON value instead of a handle,
/// like CDP `returnByValue`.
pub(crate) fn describe(scope: &Scope<'_>, value: Value<'_>, by_value: bool) -> CdpResult<Described> {
    let plain = |remote| Ok(Described { remote, keep: None });
    match value.ty() {
        JsType::Undefined => plain(RemoteObject::undefined()),
        JsType::Null => plain(RemoteObject {
            kind: "object",
            subtype: Some("null"),
            value: Some(Json::Null),
            ..RemoteObject::default()
        }),
        JsType::Boolean => {
            plain(RemoteObject { kind: "boolean", value: Some(Json::Bool(value.to_bool())), ..RemoteObject::default() })
        }
        JsType::Number => plain(RemoteObject::number(value.to_number().map_err(js_to_cdp)?)),
        JsType::String => plain(RemoteObject {
            kind: "string",
            value: Some(Json::String(value.to_string().map_err(js_to_cdp)?)),
            ..RemoteObject::default()
        }),
        JsType::BigInt => {
            let digits = value.to_string().map_err(js_to_cdp)?;
            plain(RemoteObject {
                kind: "bigint",
                unserializable_value: Some(format!("{digits}n")),
                description: Some(format!("{digits}n")),
                ..RemoteObject::default()
            })
        }
        JsType::Symbol | JsType::Object => {
            let d = scope.describe(value).map_err(js_to_cdp)?;
            let kind = static_str(&d.ty);
            let subtype = d.subtype.as_deref().map(static_str);
            if by_value && kind != "symbol" {
                // JSON.stringify semantics; unserializable results (functions,
                // cycles) become undefined as Playwright expects from Chrome's
                // "Object couldn't be returned by value" handling.
                let json = match value.to_json(0) {
                    Ok(Some(text)) => serde_json::from_str(&text).ok(),
                    _ => None,
                };
                return match json {
                    Some(json) => plain(RemoteObject { kind, subtype, value: Some(json), ..RemoteObject::default() }),
                    None => Err(CdpError::server("Object couldn't be returned by value")),
                };
            }
            Ok(Described {
                remote: RemoteObject {
                    kind,
                    subtype,
                    class_name: d.class_name,
                    description: d.description,
                    ..RemoteObject::default()
                },
                keep: Some(Persistent::new(scope, value)),
            })
        }
    }
}

/// Finishes a [`Described`] by registering its handle (if any).
pub(crate) fn register(
    described: Described,
    handles: &RefCell<HandleTable<Persistent>>,
    context: ExecutionContextId,
    group: Option<&str>,
) -> CdpResult<RemoteObject> {
    let Described { mut remote, keep } = described;
    if let Some(keep) = keep {
        remote.object_id = Some(handles.borrow_mut().insert(context, group, keep)?);
    }
    Ok(remote)
}

pub(crate) fn js_to_cdp(error: JsError) -> CdpError {
    match error {
        JsError::ContextDestroyed => CdpError::server("Execution context was destroyed."),
        other => CdpError::server(other.to_string()),
    }
}

/// CDP `exceptionDetails` for a thrown value, with a V8-style description so
/// Playwright's `getExceptionMessage`/`exceptionToError` split name, message
/// and stack as they do for Chrome. The exception gets no `objectId`: clients
/// do not release exception handles, so keeping them would only leak.
pub(crate) fn exception_details(
    scope: &Scope<'_>,
    exception: &JsException,
    exception_id: i64,
    context: Option<ExecutionContextId>,
) -> ExceptionDetails {
    let frames = exception.stack.as_deref().map(parse_jsc_stack).unwrap_or_default();
    let value = exception.value.as_ref().map(|p| p.get(scope));
    let is_error_object = value.is_some_and(|v| v.is_object()) && exception.name.is_some();
    let mut remote = match value {
        Some(v) => describe(scope, v, false).map(|d| d.remote).unwrap_or_else(|_| RemoteObject::undefined()),
        None => RemoteObject::undefined(),
    };
    if is_error_object {
        let name = exception.name.as_deref().unwrap_or("Error");
        remote.description = Some(v8_style_description(name, &exception.message, &frames));
    }
    let (line, column) = match frames.first() {
        Some(f) if f.line_number >= 0 => (f.line_number, f.column_number.max(0)),
        _ => (
            exception.line.map_or(0, |l| i64::from(l).saturating_sub(1)),
            exception.column.map_or(0, |c| i64::from(c).saturating_sub(1)),
        ),
    };
    ExceptionDetails {
        exception_id,
        text: "Uncaught".to_owned(),
        line_number: line,
        column_number: column,
        url: exception.url.clone().or_else(|| frames.first().map(|f| f.url.clone())).filter(|u| !u.is_empty()),
        stack_trace: (!frames.is_empty()).then_some(StackTrace { call_frames: frames }),
        exception: Some(remote),
        execution_context_id: context,
    }
}

/// Resolves a `callFunctionOn` argument into an engine value of `scope`'s realm.
pub(crate) fn argument<'s>(
    scope: &Scope<'s>,
    arg: &CallArgument,
    handles: &RefCell<HandleTable<Persistent>>,
    context: ExecutionContextId,
) -> CdpResult<Value<'s>> {
    Ok(match arg.decode()? {
        Argument::Undefined => scope.undefined(),
        Argument::Number(n) => scope.number(n),
        Argument::BigInt(digits) => scope
            .eval(&format!("{digits}n"), None, 1)
            .map_err(|_| CdpError::invalid_params("Couldn't parse value object in call argument"))?,
        Argument::Json(json) => json_value(scope, json)?,
        Argument::Object(id) => {
            let handles = handles.borrow();
            let (owner, handle) = handles.get(id)?;
            if owner != context {
                return Err(CdpError::server("Argument should belong to the same JavaScript world as target object"));
            }
            handle.get(scope)
        }
    })
}

fn json_value<'s>(scope: &Scope<'s>, json: &Json) -> CdpResult<Value<'s>> {
    Ok(match json {
        Json::Null => scope.null(),
        Json::Bool(b) => scope.boolean(*b),
        Json::Number(n) => scope.number(n.as_f64().unwrap_or(f64::NAN)),
        Json::String(s) => scope.string(s),
        other => scope
            .parse_json(&other.to_string())
            .ok_or_else(|| CdpError::invalid_params("Couldn't parse value object in call argument"))?,
    })
}
