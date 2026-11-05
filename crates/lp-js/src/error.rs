//! Errors crossing the Rust/JavaScript boundary.

use crate::persistent::Persistent;
use crate::realm::Scope;
use crate::string::JsString;
use crate::value::{Object, Value};
use lp_jsc_sys as sys;
use std::borrow::Cow;
use std::ptr;

pub type JsResult<T> = Result<T, JsError>;

/// An error raised by native code or a JavaScript exception being propagated.
///
/// Binding callbacks return `JsResult`; the runtime converts `Err` into a thrown
/// JavaScript exception of the matching type (`DOMException` for `Dom`).
#[derive(Debug, Clone)]
pub enum JsError {
    Type(String),
    Range(String),
    Syntax(String),
    Reference(String),
    /// A `DOMException` with the given WebIDL error name (e.g. `NotFoundError`).
    Dom {
        name: Cow<'static, str>,
        message: String,
    },
    /// A JavaScript value that was thrown and must be rethrown unchanged.
    Thrown(Persistent),
    /// Execution was terminated (watchdog / `Runtime.terminateExecution`).
    Terminated,
    /// The realm that owns the callee was disposed (e.g. navigated away).
    ContextDestroyed,
    /// A capability the running JavaScriptCore does not provide.
    Unsupported(&'static str),
}

impl JsError {
    pub fn dom(name: &'static str, message: impl Into<String>) -> JsError {
        JsError::Dom { name: Cow::Borrowed(name), message: message.into() }
    }

    pub fn type_error(message: impl Into<String>) -> JsError {
        JsError::Type(message.into())
    }

    pub(crate) fn thrown_raw(ctx: sys::JSContextRef, exc: sys::JSValueRef) -> JsError {
        // SAFETY: exc is the live exception value JSC just produced on this thread.
        let p = unsafe { Persistent::from_raw(ctx, exc) };
        match p {
            Some(p) => JsError::Thrown(p),
            None => JsError::Type("exception thrown without a live engine".into()),
        }
    }

    /// Materializes this error as a JavaScript value in `scope`'s realm.
    pub fn to_value<'s>(&self, scope: &Scope<'s>) -> Value<'s> {
        let make = |ctor: &str, msg: &str| -> Value<'s> {
            scope
                .global()
                .get(ctor)
                .ok()
                .and_then(|c| c.as_object())
                .and_then(|c| c.construct(&[scope.string(msg)]).ok())
                .map(|o| o.value())
                .unwrap_or_else(|| scope.string(msg))
        };
        match self {
            JsError::Type(m) => make("TypeError", m),
            JsError::Range(m) => make("RangeError", m),
            JsError::Syntax(m) => make("SyntaxError", m),
            JsError::Reference(m) => make("ReferenceError", m),
            JsError::Dom { name, message } => {
                let dom_exception = scope.realm_inner().dom_exception_ctor(scope);
                let quota = dom_exception.as_ref().filter(|_| name == "QuotaExceededError");
                quota
                    .and_then(|dom_exception| quota_exceeded_error_ctor(scope, dom_exception))
                    .and_then(|c| c.construct(&[scope.string(message)]).ok())
                    .or_else(|| {
                        dom_exception.and_then(|c| c.construct(&[scope.string(message), scope.string(name)]).ok())
                    })
                    .map(|o| o.value())
                    .unwrap_or_else(|| make("Error", &format!("{name}: {message}")))
            }
            JsError::Thrown(p) => p.get(scope),
            JsError::Terminated => make("Error", "Execution terminated"),
            JsError::ContextDestroyed => make("Error", "Execution context was destroyed."),
            JsError::Unsupported(what) => make("Error", &format!("Not supported by this engine build: {what}")),
        }
    }

    /// Structured details for reporting (CDP `exceptionDetails`, console).
    pub fn details(&self, scope: &Scope<'_>) -> JsException {
        let v = self.to_value(scope);
        JsException::from_value(scope, v)
    }
}

/// The realm's `QuotaExceededError` class (helpers.js), read from the global
/// like the `TypeError` and friends above: page script may replace the
/// binding, so only a constructor that still extends the realm's own
/// `DOMException` counts, else a plain DOMException of that name is thrown.
fn quota_exceeded_error_ctor<'s>(scope: &Scope<'s>, dom_exception: &Object<'s>) -> Option<Object<'s>> {
    let ctor = scope.global().get("QuotaExceededError").ok()?.as_object()?;
    (ctor.is_constructor() && ctor.prototype().strict_equals(dom_exception.value())).then_some(ctor)
}

impl std::fmt::Display for JsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JsError::Type(m) => write!(f, "TypeError: {m}"),
            JsError::Range(m) => write!(f, "RangeError: {m}"),
            JsError::Syntax(m) => write!(f, "SyntaxError: {m}"),
            JsError::Reference(m) => write!(f, "ReferenceError: {m}"),
            JsError::Dom { name, message } => write!(f, "{name}: {message}"),
            JsError::Thrown(_) => write!(f, "JavaScript exception"),
            JsError::Terminated => write!(f, "execution terminated"),
            JsError::ContextDestroyed => write!(f, "execution context was destroyed"),
            JsError::Unsupported(w) => write!(f, "unsupported: {w}"),
        }
    }
}

impl std::error::Error for JsError {}

/// A thrown JavaScript value plus the details JSC attaches to errors.
#[derive(Debug, Clone)]
pub struct JsException {
    /// `String(value)` for non-errors, `error.message` for errors.
    pub message: String,
    /// `error.name` when the value is an object with a string name.
    pub name: Option<String>,
    /// `error.stack` (JSC format: `fn@url:line:col` per line).
    pub stack: Option<String>,
    /// 1-based line (`error.line`).
    pub line: Option<u32>,
    /// 1-based column (`error.column`).
    pub column: Option<u32>,
    /// `error.sourceURL`.
    pub url: Option<String>,
    /// The thrown value itself.
    pub value: Option<Persistent>,
}

impl JsException {
    pub fn from_value(scope: &Scope<'_>, v: Value<'_>) -> JsException {
        let value = Some(Persistent::new(scope, v));
        let Some(o) = v.as_object() else {
            return JsException {
                message: v.to_string().unwrap_or_default(),
                name: None,
                stack: None,
                line: None,
                column: None,
                url: None,
                value,
            };
        };
        let str_prop = |o: &Object<'_>, k: &str| o.get(k).ok().and_then(|p| p.as_string());
        let num_prop = |o: &Object<'_>, k: &str| {
            o.get(k).ok().filter(|p| p.is_number()).and_then(|p| p.to_number().ok()).map(|n| n as u32)
        };
        JsException {
            message: str_prop(&o, "message").unwrap_or_else(|| v.to_string().unwrap_or_default()),
            name: str_prop(&o, "name"),
            stack: str_prop(&o, "stack"),
            line: num_prop(&o, "line"),
            column: num_prop(&o, "column"),
            url: str_prop(&o, "sourceURL"),
            value,
        }
    }

    /// `Name: message` as a browser would print it.
    pub fn text(&self) -> String {
        match &self.name {
            Some(n) if !self.message.is_empty() => format!("{n}: {}", self.message),
            Some(n) => n.clone(),
            None => self.message.clone(),
        }
    }
}

/// Writes `err` into a callback's exception out-parameter.
///
/// # Safety
/// `exc` must be the exception pointer JSC passed to the current callback.
pub(crate) unsafe fn throw_into(scope: &Scope<'_>, err: &JsError, exc: *mut sys::JSValueRef) {
    if !exc.is_null() {
        unsafe { *exc = err.to_value(scope).raw };
    }
}

/// Throws a plain `Error` with `msg` when no realm scope is available.
///
/// # Safety
/// `ctx` must be live; `exc` as for [`throw_into`].
pub(crate) unsafe fn throw_message(ctx: sys::JSContextRef, msg: &str, exc: *mut sys::JSValueRef) {
    if exc.is_null() {
        return;
    }
    let s = JsString::new(msg);
    unsafe {
        let arg = sys::JSValueMakeString(ctx, s.as_raw());
        let e = sys::JSObjectMakeError(ctx, 1, &arg, ptr::null_mut());
        *exc = if e.is_null() { arg } else { e as sys::JSValueRef };
    }
}
