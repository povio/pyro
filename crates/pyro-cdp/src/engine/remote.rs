//! Conversion between engine values and CDP `Runtime.RemoteObject`s.
//!
//! Conversion never holds a borrow of the handle table while JavaScript runs:
//! [`describe`] calls into the engine and returns the value to keep, and the
//! caller inserts it afterwards. Running JavaScript can drain microtasks,
//! which may run other pending CDP replies that use the same table.

use std::cell::RefCell;
use std::rc::Rc;

use pyro_js::{JsError, JsException, JsType, Persistent, Scope, Value};
use serde_json::value::RawValue;
use serde_json::Value as Json;

use crate::msg::{CdpError, CdpResult};
use crate::runtime_protocol::{
    parse_jsc_stack, v8_style_description, Argument, CallArgument, ExceptionDetails, ExecutionContextId, HandleTable,
    RemoteObject, RemoteValue, StackTrace,
};

/// A converted value plus the engine reference to store if the client should
/// get an `objectId`.
/// A session's remote-object table. Handles are shared (`Rc`) so a command
/// can keep its target object alive across a call without re-protecting it.
pub(crate) type Handles = RefCell<HandleTable<Rc<Persistent>>>;

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
            value: Some(Json::Null.into()),
            ..RemoteObject::default()
        }),
        JsType::Boolean => plain(RemoteObject {
            kind: "boolean",
            value: Some(Json::Bool(value.to_bool()).into()),
            ..RemoteObject::default()
        }),
        JsType::Number => plain(RemoteObject::number(value.to_number().map_err(js_to_cdp)?)),
        JsType::String => plain(RemoteObject {
            kind: "string",
            value: Some(Json::String(value.to_string().map_err(js_to_cdp)?).into()),
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
            if !by_value {
                // Native fast path for the handle-returning cases Playwright hits
                // most: DOM wrappers and ordinary objects. Everything else (arrays,
                // errors, promises, collections, functions) takes the JS helper.
                if let Some(name) = scope.host_class_name(value) {
                    let node = pyro_webapi::node_from_value(scope, value)
                        .and_then(|(page, id)| page.dom.borrow().node_name(id).ok())
                        .map(|n| n.to_ascii_lowercase());
                    return Ok(Described {
                        remote: RemoteObject {
                            kind: "object",
                            subtype: node.is_some().then_some("node"),
                            class_name: Some(name.to_owned()),
                            description: Some(node.unwrap_or_else(|| name.to_owned())),
                            ..RemoteObject::default()
                        },
                        keep: Some(Persistent::new(scope, value)),
                    });
                }
                if scope.is_plain_object(value) {
                    return Ok(Described {
                        remote: RemoteObject {
                            kind: "object",
                            class_name: Some("Object".to_owned()),
                            description: Some("Object".to_owned()),
                            ..RemoteObject::default()
                        },
                        keep: Some(Persistent::new(scope, value)),
                    });
                }
            }
            let d = scope.describe(value).map_err(js_to_cdp)?;
            let kind = static_str(&d.ty);
            let subtype = d.subtype.as_deref().map(static_str);
            if by_value && kind != "symbol" {
                // JSON.stringify semantics; unserializable results (functions,
                // cycles) become undefined as Playwright expects from Chrome's
                // "Object couldn't be returned by value" handling. The engine's
                // JSON text goes to the wire as is (key order kept, like Chrome);
                // the check only enforces what parsing it used to (depth limit).
                let json = match value.to_json(0) {
                    Ok(Some(text)) => serde_json::value::RawValue::from_string(text).ok(),
                    _ => None,
                };
                return match json {
                    Some(json) => plain(RemoteObject {
                        kind,
                        subtype,
                        value: Some(RemoteValue::Raw(json)),
                        ..RemoteObject::default()
                    }),
                    None => Err(CdpError::server("Object couldn't be returned by value")),
                };
            }
            let description = if subtype == Some("error") { error_description(value) } else { None };
            Ok(Described {
                remote: RemoteObject {
                    kind,
                    subtype,
                    class_name: d.class_name,
                    description: description.or(d.description),
                    ..RemoteObject::default()
                },
                keep: Some(Persistent::new(scope, value)),
            })
        }
    }
}

/// Chrome's description of an Error: `name: message` as they are now (like
/// Error.prototype.toString), then the stack in V8's `    at` form.
/// JavaScriptCore's `stack` holds only frames, so using it alone loses the
/// message (and console.error(err) text with it).
fn error_description(value: Value<'_>) -> Option<String> {
    let object = value.as_object()?;
    let read = |key: &str| -> String {
        object.get(key).ok().filter(|v| !v.is_undefined()).and_then(|v| v.to_string().ok()).unwrap_or_default()
    };
    let name = object
        .get("name")
        .ok()
        .filter(|v| !v.is_undefined())
        .map_or("Error".to_owned(), |v| v.to_string().unwrap_or_default());
    let message = read("message");
    let frames = parse_jsc_stack(&read("stack"));
    Some(if name.is_empty() {
        v8_style_description(&message, "", &frames)
    } else {
        v8_style_description(&name, &message, &frames)
    })
}

/// A promise's `[[PromiseState]]` and, once settled, `[[PromiseResult]]`.
/// `None` for anything else. JavaScriptCore offers no synchronous view of a
/// promise's state: reactions attached by the probe run when its native call
/// returns to the top level (microtasks drain then), so a settled promise
/// reads as settled unless JavaScript is already on the stack (inside a
/// microtask it reads "pending"). Attaching the reaction marks a rejected
/// promise as handled, which Chrome's inspector does not.
pub(crate) fn promise_state(scope: &Scope<'_>, value: Value<'_>) -> Option<(String, Option<Persistent>)> {
    let reader = scope.promise_probe(value).ok().flatten()?;
    let pair = reader.call(None, &[]).ok()?.as_object()?;
    let state = pair.get_index(0).ok()?.as_string()?;
    let result = (state != "pending").then(|| pair.get_index(1).ok().map(|v| Persistent::new(scope, v))).flatten();
    Some((state, result))
}

/// Adds a `Runtime.ObjectPreview` to an object's remote description
/// (`generatePreview`, console arguments). Best effort: a preview that
/// cannot be built is left out.
pub(crate) fn add_preview(scope: &Scope<'_>, value: Value<'_>, remote: &mut RemoteObject) {
    if remote.kind != "object" || remote.subtype == Some("null") {
        return;
    }
    let promise = if remote.subtype == Some("promise") {
        promise_state(scope, value).and_then(|(state, result)| {
            let result = result.map_or_else(|| scope.undefined(), |r| r.get(scope));
            let pair = scope.array(&[scope.string(&state), result]).ok()?;
            Some(Persistent::new(scope, pair.value()))
        })
    } else {
        None
    };
    let promise = promise.as_ref().map(|p| p.get(scope));
    if let Ok(text) = scope.object_preview(value, remote.description.as_deref(), promise) {
        remote.preview = serde_json::value::RawValue::from_string(text).ok().map(RemoteValue::Raw);
    }
}

/// Finishes a [`Described`] by registering its handle (if any).
pub(crate) fn register(
    described: Described,
    handles: &Handles,
    context: ExecutionContextId,
    group: Option<&str>,
) -> CdpResult<RemoteObject> {
    let Described { remote, keep } = described;
    register_shared(remote, keep.map(Rc::new), handles, context, group)
}

/// [`register`] for a handle that several sessions share (console arguments).
pub(crate) fn register_shared(
    mut remote: RemoteObject,
    keep: Option<Rc<Persistent>>,
    handles: &Handles,
    context: ExecutionContextId,
    group: Option<&str>,
) -> CdpResult<RemoteObject> {
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
    handles: &Handles,
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

/// The engine value of a JSON argument. Scalars are converted natively;
/// objects and arrays go to the engine's `JSON.parse` as sent, so no value
/// tree is built and key order is kept (as in Chrome).
fn json_value<'s>(scope: &Scope<'s>, json: &RawValue) -> CdpResult<Value<'s>> {
    let invalid = || CdpError::invalid_params("Couldn't parse value object in call argument");
    let text = json.get();
    Ok(match text.as_bytes().first() {
        Some(b'n') => scope.null(),
        Some(b't') => scope.boolean(true),
        Some(b'f') => scope.boolean(false),
        // The parser that validated the message, so numbers round the same.
        Some(b'-' | b'0'..=b'9') => scope.number(serde_json::from_str::<f64>(text).map_err(|_| invalid())?),
        Some(b'"') => scope.string(&serde_json::from_str::<std::borrow::Cow<'_, str>>(text).map_err(|_| invalid())?),
        _ => scope.parse_json(text).ok_or_else(invalid)?,
    })
}
