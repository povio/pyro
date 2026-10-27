//! Engine-independent pieces of the `Runtime` domain: wire types for remote
//! objects and exceptions, `callFunctionOn` argument decoding, and the
//! per-session table that maps `objectId`s to engine handles.
//!
//! Upstream delegates all of this to v8_inspector; `engine::runtime` builds
//! the domain on lp-js (JavaScriptCore) using these pieces. Shapes follow the CDP `Runtime` domain as
//! consumed by Playwright 1.59 (`crExecutionContext.js`, `crProtocolHelper.js`).
//! The engine-backed domain lives in `engine::runtime`.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::msg::{CdpError, CdpResult};

pub type ExecutionContextId = i64;

#[derive(Debug, Clone, Default, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RemoteObject {
    #[serde(rename = "type")]
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtype: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<RemoteValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unserializable_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object_id: Option<String>,
}

/// `RemoteObject.value`: a JSON value, or JSON text already produced by the
/// engine (by-value results), written to the wire as is.
#[derive(Debug, Clone)]
pub enum RemoteValue {
    Json(Value),
    Raw(Box<serde_json::value::RawValue>),
}

impl From<Value> for RemoteValue {
    fn from(v: Value) -> Self {
        RemoteValue::Json(v)
    }
}

impl PartialEq for RemoteValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (RemoteValue::Json(a), RemoteValue::Json(b)) => a == b,
            (RemoteValue::Raw(a), RemoteValue::Raw(b)) => a.get() == b.get(),
            _ => false,
        }
    }
}

impl Serialize for RemoteValue {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            RemoteValue::Json(v) => v.serialize(s),
            RemoteValue::Raw(r) => r.serialize(s),
        }
    }
}

impl RemoteObject {
    pub fn undefined() -> Self {
        Self { kind: "undefined", ..Self::default() }
    }

    /// A primitive or by-value result. Numbers that JSON cannot carry are
    /// reported through `unserializableValue` as CDP requires.
    pub fn number(n: f64) -> Self {
        let unserializable = if n.is_nan() {
            Some("NaN")
        } else if n == f64::INFINITY {
            Some("Infinity")
        } else if n == f64::NEG_INFINITY {
            Some("-Infinity")
        } else if n == 0.0 && n.is_sign_negative() {
            Some("-0")
        } else {
            None
        };
        let description = match unserializable {
            Some(u) => u.to_owned(),
            None => format_number(n),
        };
        Self {
            kind: "number",
            value: unserializable.is_none().then(|| number_value(n).into()),
            unserializable_value: unserializable.map(str::to_owned),
            description: Some(description),
            ..Self::default()
        }
    }
}

/// ECMAScript `Number::toString(10)` (shortest round-trip digits with the
/// ECMAScript tie rule and exponent thresholds), via `ryu-js`.
pub fn format_number(n: f64) -> String {
    ryu_js::Buffer::new().format(n).to_owned()
}

fn number_value(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_991.0 {
        Value::from(n as i64)
    } else {
        serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number)
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CallFrame {
    pub function_name: String,
    pub script_id: String,
    pub url: String,
    pub line_number: i64,
    pub column_number: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StackTrace {
    pub call_frames: Vec<CallFrame>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ExceptionDetails {
    pub exception_id: i64,
    pub text: String,
    pub line_number: i64,
    pub column_number: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stack_trace: Option<StackTrace>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exception: Option<RemoteObject>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_context_id: Option<ExecutionContextId>,
}

/// Parses a JavaScriptCore `Error.prototype.stack` (`fn@url:line:col` per
/// line) into CDP call frames. JSC line numbers are 1-based; CDP's are 0-based.
pub fn parse_jsc_stack(stack: &str) -> Vec<CallFrame> {
    stack
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| {
            let (function_name, location) = match line.split_once('@') {
                // Native frames carry no source location.
                Some((f, "[native code]")) => (f, ""),
                Some((f, loc)) => (f, loc),
                None => (line, ""),
            };
            let (url, line_number, column_number) = split_location(location);
            CallFrame {
                function_name: function_name.to_owned(),
                script_id: String::new(),
                url: url.to_owned(),
                line_number: line_number.map_or(-1, |n| n - 1),
                column_number: column_number.map_or(-1, |n| n - 1),
            }
        })
        .collect()
}

fn split_location(location: &str) -> (&str, Option<i64>, Option<i64>) {
    let mut parts = location.rsplitn(3, ':');
    let (last, mid, rest) = (parts.next(), parts.next(), parts.next());
    match (last.and_then(|c| c.parse().ok()), mid.and_then(|l| l.parse().ok()), rest) {
        (Some(col), Some(line), Some(url)) => (url, Some(line), Some(col)),
        _ => (location, None, None),
    }
}

/// V8-style description (`Name: message\n    at fn (url:line:col)`), the
/// format Playwright splits into name/message/stack for `pageerror` and
/// evaluation errors. Line/column are printed 1-based like V8.
pub fn v8_style_description(name: &str, message: &str, frames: &[CallFrame]) -> String {
    let mut out = if message.is_empty() { name.to_owned() } else { format!("{name}: {message}") };
    for f in frames {
        let function = if f.function_name.is_empty() { "<anonymous>" } else { &f.function_name };
        if f.url.is_empty() {
            out.push_str(&format!("\n    at {function} (native)"));
        } else {
            out.push_str(&format!("\n    at {function} ({}:{}:{})", f.url, f.line_number + 1, f.column_number + 1));
        }
    }
    out
}

/// An argument of `Runtime.callFunctionOn`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CallArgument {
    /// `Some(Value::Null)` for an explicit `"value": null`, `None` when absent.
    #[serde(default, deserialize_with = "present")]
    pub value: Option<Value>,
    #[serde(default)]
    pub unserializable_value: Option<String>,
    #[serde(default)]
    pub object_id: Option<String>,
}

fn present<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<Value>, D::Error> {
    Value::deserialize(d).map(Some)
}

/// Decoded argument, ready to convert into an engine value.
#[derive(Debug, Clone, PartialEq)]
pub enum Argument<'a> {
    Undefined,
    Json(&'a Value),
    Number(f64),
    BigInt(&'a str),
    Object(&'a str),
}

impl CallArgument {
    pub fn decode(&self) -> CdpResult<Argument<'_>> {
        match (&self.value, &self.unserializable_value, &self.object_id) {
            (None, None, None) => Ok(Argument::Undefined),
            (Some(v), None, None) => Ok(Argument::Json(v)),
            (None, Some(u), None) => parse_unserializable(u),
            (None, None, Some(id)) => Ok(Argument::Object(id)),
            _ => Err(CdpError::invalid_params("Only one of value, unserializableValue or objectId may be set")),
        }
    }
}

fn parse_unserializable(u: &str) -> CdpResult<Argument<'_>> {
    match u {
        "NaN" => Ok(Argument::Number(f64::NAN)),
        "Infinity" => Ok(Argument::Number(f64::INFINITY)),
        "-Infinity" => Ok(Argument::Number(f64::NEG_INFINITY)),
        "-0" => Ok(Argument::Number(-0.0)),
        _ => match u.strip_suffix('n') {
            Some(digits) if is_bigint_literal(digits) => Ok(Argument::BigInt(digits)),
            _ => Err(CdpError::invalid_params("Couldn't parse value object in call argument")),
        },
    }
}

fn is_bigint_literal(digits: &str) -> bool {
    let unsigned = digits.strip_prefix('-').unwrap_or(digits);
    !unsigned.is_empty() && unsigned.bytes().all(|b| b.is_ascii_digit())
}

/// Maps `objectId`s to engine handles for one CDP session.
///
/// Ids are `"<executionContextId>.<n>"`, so a stale id from a destroyed
/// context is recognizably invalid rather than aliasing a newer object.
/// The table is bounded so a client that never releases handles gets errors
/// instead of growing memory without limit.
pub struct HandleTable<H> {
    entries: HashMap<String, Entry<H>>,
    next: u64,
    budget: HandleBudget,
}

/// Remote-object handles a whole connection may retain, shared by the
/// handle tables of all its sessions (the session count alone does not bound
/// the aggregate). Single-threaded: lives on the browser thread.
#[derive(Clone)]
pub struct HandleBudget(Rc<HandleBudgetInner>);

struct HandleBudgetInner {
    limit: usize,
    used: Cell<usize>,
}

impl HandleBudget {
    /// Default per connection. 100,000 retained small-object handles
    /// measured +28.3..28.8 MiB process footprint on real JavaScriptCore
    /// (bench/results/memory-caps-cdf2d33). Larger retained objects cost
    /// what they hold. Playwright releases handles it no longer needs;
    /// reaching the limit is a client leak and fails explicitly.
    pub const DEFAULT_LIMIT: usize = 100_000;

    pub fn new(limit: usize) -> Self {
        Self(Rc::new(HandleBudgetInner { limit, used: Cell::new(0) }))
    }

    pub fn used(&self) -> usize {
        self.0.used.get()
    }

    /// True while a handle table (or any other holder) shares this budget.
    pub fn is_shared(&self) -> bool {
        Rc::strong_count(&self.0) > 1
    }

    fn take(&self) -> bool {
        let used = self.0.used.get();
        if used >= self.0.limit {
            return false;
        }
        self.0.used.set(used + 1);
        true
    }

    fn give_back(&self, n: usize) {
        self.0.used.set(self.0.used.get().saturating_sub(n));
    }
}

struct Entry<H> {
    context: ExecutionContextId,
    group: Option<String>,
    handle: H,
}

impl<H> HandleTable<H> {
    pub fn new(budget: HandleBudget) -> Self {
        Self { entries: HashMap::new(), next: 1, budget }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn insert(&mut self, context: ExecutionContextId, group: Option<&str>, handle: H) -> CdpResult<String> {
        if !self.budget.take() {
            return Err(CdpError::server("Too many remote objects; release unused handles"));
        }
        let id = format!("{context}.{}", self.next);
        self.next += 1;
        self.entries.insert(id.clone(), Entry { context, group: group.map(str::to_owned), handle });
        Ok(id)
    }

    pub fn get(&self, object_id: &str) -> CdpResult<(ExecutionContextId, &H)> {
        self.entries
            .get(object_id)
            .map(|e| (e.context, &e.handle))
            .ok_or_else(|| CdpError::server("Could not find object with given id"))
    }

    pub fn group_of(&self, object_id: &str) -> Option<&str> {
        self.entries.get(object_id).and_then(|e| e.group.as_deref())
    }

    pub fn release(&mut self, object_id: &str) -> Option<H> {
        let entry = self.entries.remove(object_id)?;
        self.budget.give_back(1);
        Some(entry.handle)
    }

    pub fn release_group(&mut self, group: &str) -> usize {
        self.release_where(|e| e.group.as_deref() == Some(group))
    }

    /// Drops every handle of a destroyed execution context.
    pub fn release_context(&mut self, context: ExecutionContextId) -> usize {
        self.release_where(|e| e.context == context)
    }

    pub fn clear(&mut self) {
        self.budget.give_back(self.entries.len());
        self.entries.clear();
    }

    fn release_where(&mut self, mut pred: impl FnMut(&Entry<H>) -> bool) -> usize {
        let before = self.entries.len();
        self.entries.retain(|_, e| !pred(e));
        let released = before - self.entries.len();
        self.budget.give_back(released);
        released
    }
}

impl<H> Drop for HandleTable<H> {
    fn drop(&mut self) {
        self.budget.give_back(self.entries.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_use_unserializable_values_when_needed() {
        assert_eq!(
            serde_json::to_value(RemoteObject::number(f64::NAN)).unwrap(),
            json!({"type":"number","unserializableValue":"NaN","description":"NaN"})
        );
        assert_eq!(serde_json::to_value(RemoteObject::number(-0.0)).unwrap()["unserializableValue"], "-0");
        assert_eq!(
            serde_json::to_value(RemoteObject::number(3.0)).unwrap(),
            json!({"type":"number","value":3,"description":"3"})
        );
        assert_eq!(serde_json::to_value(RemoteObject::number(1.5)).unwrap()["value"], json!(1.5));
        for (n, js) in [
            (1e21, "1e+21"),
            (1e20, "100000000000000000000"),
            (123.456, "123.456"),
            (0.000001, "0.000001"),
            (1e-7, "1e-7"),
            (1.5e-7, "1.5e-7"),
            (-2.5e30, "-2.5e+30"),
            (0.1, "0.1"),
            (-42.0, "-42"),
            (f64::from_bits(0x430a8e5672bc7312), "934356417220194.2"),
        ] {
            assert_eq!(format_number(n), js, "{n:e}");
        }
        assert_eq!(serde_json::to_value(RemoteObject::undefined()).unwrap(), json!({"type":"undefined"}));
    }

    #[test]
    fn parses_jsc_stacks_into_zero_based_frames() {
        let frames = parse_jsc_stack(
            "foo@http://h/a.js:10:5\n@http://h/b.js:1:1\nforEach@[native code]\nglobal code@http://h/c.js:3:7\n",
        );
        assert_eq!(frames.len(), 4);
        assert_eq!(
            (frames[0].function_name.as_str(), frames[0].url.as_str(), frames[0].line_number, frames[0].column_number),
            ("foo", "http://h/a.js", 9, 4)
        );
        assert_eq!(frames[1].function_name, "");
        assert_eq!((frames[2].url.as_str(), frames[2].line_number), ("", -1));
        assert_eq!(
            v8_style_description("Error", "x", &frames[2..3]),
            "Error: x\n    at forEach (native)",
            "native frames are not printed as URLs"
        );
        assert_eq!(frames[3].function_name, "global code");
        assert_eq!(
            v8_style_description("TypeError", "x is not a function", &frames[..2]),
            "TypeError: x is not a function\n    at foo (http://h/a.js:10:5)\n    at <anonymous> (http://h/b.js:1:1)"
        );
    }

    #[test]
    fn decodes_call_arguments() {
        let arg = |v: Value| serde_json::from_value::<CallArgument>(v).unwrap();
        assert_eq!(arg(json!({})).decode().unwrap(), Argument::Undefined);
        assert_eq!(
            arg(json!({"value":null})).decode().unwrap(),
            Argument::Json(&Value::Null),
            "explicit null is not undefined"
        );
        let a = arg(json!({"value":{"a":1}}));
        assert_eq!(a.decode().unwrap(), Argument::Json(&json!({"a":1})));
        assert!(
            matches!(arg(json!({"unserializableValue":"-0"})).decode().unwrap(), Argument::Number(n) if n == 0.0 && n.is_sign_negative())
        );
        assert_eq!(arg(json!({"unserializableValue":"-12n"})).decode().unwrap(), Argument::BigInt("-12"));
        assert_eq!(arg(json!({"objectId":"1.2"})).decode().unwrap(), Argument::Object("1.2"));
        assert!(arg(json!({"unserializableValue":"1e3n"})).decode().is_err());
        assert!(arg(json!({"value":1,"objectId":"1.2"})).decode().is_err());
    }

    #[test]
    fn handle_budget_is_shared_and_returned() {
        let budget = HandleBudget::new(3);
        assert!(!budget.is_shared());
        let mut a = HandleTable::new(budget.clone());
        let mut b = HandleTable::new(budget.clone());
        assert!(budget.is_shared());
        let a1 = a.insert(1, Some("g"), "a1").unwrap();
        a.insert(1, None, "a2").unwrap();
        b.insert(2, None, "b1").unwrap();
        assert!(b.insert(2, None, "b2").is_err(), "aggregate across tables is bounded");
        assert_eq!(budget.used(), 3);
        a.release(&a1);
        b.insert(2, None, "b2").unwrap();
        a.clear();
        assert_eq!(budget.used(), 2);
        b.release_context(2);
        assert_eq!(budget.used(), 0);
        b.insert(2, Some("g"), "b3").unwrap();
        drop(b);
        assert_eq!(budget.used(), 0, "dropping a table (session detach) returns its handles");
    }

    #[test]
    fn handle_table_groups_contexts_and_limits() {
        let mut t = HandleTable::new(HandleBudget::new(3));
        let a = t.insert(1, Some("g"), "a").unwrap();
        let b = t.insert(1, None, "b").unwrap();
        let c = t.insert(2, Some("g"), "c").unwrap();
        assert!(t.insert(2, None, "d").is_err(), "bounded");
        assert_eq!(t.get(&b).unwrap(), (1, &"b"));
        assert_eq!(t.release_group("g"), 2);
        assert!(t.get(&a).is_err() && t.get(&c).is_err());
        assert_eq!(t.release_context(1), 1);
        assert!(t.is_empty());
        let d = t.insert(3, None, "d").unwrap();
        assert_ne!(d, a, "ids are never reused");
        assert_eq!(t.release(&d), Some("d"));
        assert_eq!(t.release(&d), None);
    }
}
