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
    /// `Runtime.ObjectPreview` (JSON text from the engine), when asked for.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview: Option<RemoteValue>,
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
///
/// Storage is a slab: `n` encodes the slot index (low 32 bits, 1-based) and
/// the slot's generation (high 32 bits), so a lookup parses two integers and
/// indexes a vector, and a released slot's old ids stay invalid when it is
/// reused. Every entry is threaded on two intrusive lists, one per execution
/// context and one per object group, so releasing a context or group visits
/// only the entries it releases.
pub struct HandleTable<H> {
    slots: Vec<Slot<H>>,
    free: Vec<u32>,
    len: usize,
    /// First entry of each execution context's list.
    contexts: HashMap<ExecutionContextId, u32>,
    /// Interned object groups; a group's slot is freed with its last entry.
    groups: Vec<Option<Group>>,
    free_groups: Vec<u32>,
    group_ids: HashMap<Rc<str>, u32>,
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

const NIL: u32 = u32::MAX;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Link {
    prev: u32,
    next: u32,
}

const UNLINKED: Link = Link { prev: NIL, next: NIL };

struct Slot<H> {
    generation: u32,
    entry: Option<Entry<H>>,
}

struct Entry<H> {
    context: ExecutionContextId,
    /// Index into `groups`, or `NIL`.
    group: u32,
    handle: H,
    in_context: Link,
    in_group: Link,
}

struct Group {
    name: Rc<str>,
    head: u32,
    len: usize,
}

/// What [`HandleTable::lookup`] finds for an `objectId`.
pub struct Found<'a, H> {
    pub context: ExecutionContextId,
    pub handle: &'a H,
    pub group: Option<&'a Rc<str>>,
}

/// Splits `"<context>.<n>"` into its two integers, accepting only the
/// canonical spelling the table hands out (no `+`, no leading zeros).
fn parse_object_id(id: &str) -> Option<(ExecutionContextId, u64)> {
    fn digits(s: &str) -> bool {
        !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) && (s.len() == 1 || !s.starts_with('0'))
    }
    let (context, n) = id.split_once('.')?;
    let unsigned = context.strip_prefix('-').unwrap_or(context);
    if !digits(unsigned) || !digits(n) || context == "-0" {
        return None;
    }
    Some((context.parse().ok()?, n.parse().ok()?))
}

fn not_found() -> CdpError {
    CdpError::server("Could not find object with given id")
}

fn too_many() -> CdpError {
    CdpError::server("Too many remote objects; release unused handles")
}

impl<H> HandleTable<H> {
    pub fn new(budget: HandleBudget) -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            len: 0,
            contexts: HashMap::new(),
            groups: Vec::new(),
            free_groups: Vec::new(),
            group_ids: HashMap::new(),
            budget,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn insert(&mut self, context: ExecutionContextId, group: Option<&str>, handle: H) -> CdpResult<String> {
        if !self.budget.take() {
            return Err(too_many());
        }
        let index = match self.free.pop() {
            Some(index) => index,
            None => {
                let Some(index) = u32::try_from(self.slots.len()).ok().filter(|&i| i < NIL - 1) else {
                    self.budget.give_back(1);
                    return Err(too_many());
                };
                self.slots.push(Slot { generation: 0, entry: None });
                index
            }
        };
        let group = group.map_or(NIL, |name| self.intern_group(name));
        let mut entry = Entry { context, group, handle, in_context: UNLINKED, in_group: UNLINKED };
        let head = self.contexts.entry(context).or_insert(NIL);
        entry.in_context.next = std::mem::replace(head, index);
        if entry.in_context.next != NIL {
            self.entry_mut(entry.in_context.next).in_context.prev = index;
        }
        if group != NIL {
            let g = self.groups[group as usize].as_mut().expect("interned group");
            entry.in_group.next = std::mem::replace(&mut g.head, index);
            g.len += 1;
            if entry.in_group.next != NIL {
                self.entry_mut(entry.in_group.next).in_group.prev = index;
            }
        }
        let slot = &mut self.slots[index as usize];
        slot.entry = Some(entry);
        self.len += 1;
        let n = (u64::from(slot.generation) << 32) | u64::from(index + 1);
        Ok(format!("{context}.{n}"))
    }

    fn intern_group(&mut self, name: &str) -> u32 {
        if let Some(&id) = self.group_ids.get(name) {
            return id;
        }
        let name: Rc<str> = Rc::from(name);
        let group = Group { name: name.clone(), head: NIL, len: 0 };
        let id = match self.free_groups.pop() {
            Some(id) => {
                self.groups[id as usize] = Some(group);
                id
            }
            None => {
                self.groups.push(Some(group));
                (self.groups.len() - 1) as u32
            }
        };
        self.group_ids.insert(name, id);
        id
    }

    fn entry_mut(&mut self, index: u32) -> &mut Entry<H> {
        self.slots[index as usize].entry.as_mut().expect("linked entries are live")
    }

    fn group_mut(&mut self, group: u32) -> &mut Group {
        self.groups[group as usize].as_mut().expect("groups with entries are live")
    }

    /// The live slot an id names, if any.
    fn index_of(&self, object_id: &str) -> Option<u32> {
        let (context, n) = parse_object_id(object_id)?;
        let generation = u32::try_from(n >> 32).ok()?;
        let index = (n as u32).checked_sub(1)?;
        let slot = self.slots.get(index as usize)?;
        let entry = slot.entry.as_ref()?;
        (slot.generation == generation && entry.context == context).then_some(index)
    }

    /// The context, handle and object group of `object_id`.
    pub fn lookup(&self, object_id: &str) -> CdpResult<Found<'_, H>> {
        let index = self.index_of(object_id).ok_or_else(not_found)?;
        let entry = self.slots[index as usize].entry.as_ref().expect("index_of checks liveness");
        let group = (entry.group != NIL).then(|| &self.groups[entry.group as usize].as_ref().expect("live group").name);
        Ok(Found { context: entry.context, handle: &entry.handle, group })
    }

    pub fn get(&self, object_id: &str) -> CdpResult<(ExecutionContextId, &H)> {
        self.lookup(object_id).map(|f| (f.context, f.handle))
    }

    pub fn group_of(&self, object_id: &str) -> Option<&str> {
        self.lookup(object_id).ok().and_then(|f| f.group).map(|g| &**g)
    }

    pub fn release(&mut self, object_id: &str) -> Option<H> {
        let index = self.index_of(object_id)?;
        let handle = self.remove(index, true);
        self.budget.give_back(1);
        Some(handle)
    }

    pub fn release_group(&mut self, group: &str) -> usize {
        let Some(&id) = self.group_ids.get(group) else { return 0 };
        let mut index = self.group_mut(id).head;
        let mut released = 0;
        while index != NIL {
            let next = self.entry_mut(index).in_group.next;
            drop(self.remove(index, true));
            released += 1;
            index = next;
        }
        self.budget.give_back(released);
        released
    }

    /// Drops every handle of a destroyed execution context.
    pub fn release_context(&mut self, context: ExecutionContextId) -> usize {
        let Some(mut index) = self.contexts.remove(&context) else { return 0 };
        let mut released = 0;
        while index != NIL {
            let next = self.entry_mut(index).in_context.next;
            drop(self.remove(index, false));
            released += 1;
            index = next;
        }
        self.budget.give_back(released);
        released
    }

    pub fn clear(&mut self) {
        self.budget.give_back(self.len);
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.entry.take().is_some() {
                slot.generation = slot.generation.wrapping_add(1);
                self.free.push(index as u32);
            }
        }
        self.len = 0;
        self.contexts.clear();
        self.groups.clear();
        self.free_groups.clear();
        self.group_ids.clear();
    }

    /// Frees slot `index` and unlinks it from its group list and, unless the
    /// caller is dropping that whole list from its head, its context list.
    fn remove(&mut self, index: u32, unlink_context: bool) -> H {
        let slot = &mut self.slots[index as usize];
        let entry = slot.entry.take().expect("removing a live entry");
        slot.generation = slot.generation.wrapping_add(1);
        self.free.push(index);
        self.len -= 1;
        let Link { prev, next } = entry.in_context;
        if unlink_context {
            match (prev, next) {
                (NIL, NIL) => drop(self.contexts.remove(&entry.context)),
                (NIL, next) => drop(self.contexts.insert(entry.context, next)),
                (prev, next) => self.entry_mut(prev).in_context.next = next,
            }
        }
        if next != NIL {
            self.entry_mut(next).in_context.prev = if unlink_context { prev } else { NIL };
        }
        if entry.group != NIL {
            let Link { prev, next } = entry.in_group;
            if prev == NIL {
                self.group_mut(entry.group).head = next;
            } else {
                self.entry_mut(prev).in_group.next = next;
            }
            if next != NIL {
                self.entry_mut(next).in_group.prev = prev;
            }
            let g = self.group_mut(entry.group);
            g.len -= 1;
            if g.len == 0 {
                let name = self.groups[entry.group as usize].take().expect("live group").name;
                self.group_ids.remove(&name);
                self.free_groups.push(entry.group);
            }
        }
        entry.handle
    }

    /// Recomputes the context and group lists from the slots and checks the
    /// maintained ones against them.
    #[cfg(test)]
    fn validate(&self) {
        use std::collections::BTreeSet;
        let live: Vec<u32> = (0..self.slots.len() as u32).filter(|&i| self.slots[i as usize].entry.is_some()).collect();
        assert_eq!(live.len(), self.len);
        let mut by_context: HashMap<ExecutionContextId, BTreeSet<u32>> = HashMap::new();
        let mut by_group: HashMap<u32, BTreeSet<u32>> = HashMap::new();
        for &i in &live {
            let e = self.slots[i as usize].entry.as_ref().unwrap();
            by_context.entry(e.context).or_default().insert(i);
            if e.group != NIL {
                by_group.entry(e.group).or_default().insert(i);
            }
        }
        let walk = |mut i: u32, link: fn(&Entry<H>) -> Link| {
            let mut seen = BTreeSet::new();
            let mut prev = NIL;
            while i != NIL {
                let e = self.slots[i as usize].entry.as_ref().expect("lists hold live entries");
                assert_eq!(link(e).prev, prev, "back link");
                assert!(seen.insert(i), "no cycles");
                prev = i;
                i = link(e).next;
            }
            seen
        };
        assert_eq!(self.contexts.len(), by_context.len());
        for (context, expected) in &by_context {
            assert_eq!(&walk(self.contexts[context], |e| e.in_context), expected);
        }
        assert_eq!(self.groups.iter().filter(|g| g.is_some()).count(), by_group.len());
        assert_eq!(self.group_ids.len(), by_group.len());
        for (group, expected) in &by_group {
            let g = self.groups[*group as usize].as_ref().unwrap();
            assert_eq!(g.len, expected.len());
            assert_eq!(self.group_ids[&g.name], *group);
            assert_eq!(&walk(g.head, |e| e.in_group), expected);
        }
        let free: BTreeSet<u32> = self.free.iter().copied().collect();
        assert_eq!(free.len(), self.free.len());
        assert_eq!(free.len() + live.len(), self.slots.len());
    }
}

impl<H> Drop for HandleTable<H> {
    fn drop(&mut self) {
        self.budget.give_back(self.len);
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
        t.validate();
    }

    #[test]
    fn object_ids_parse_strictly_and_stay_invalid_after_release() {
        let mut t = HandleTable::new(HandleBudget::new(10));
        let a = t.insert(7, Some("g"), "a").unwrap();
        assert_eq!(a, "7.1", "first generation ids read like before");
        let lookup = t.lookup(&a).unwrap();
        assert_eq!((lookup.context, *lookup.handle, lookup.group.map(|g| &**g)), (7, "a", Some("g")));
        for bad in ["7.01", "07.1", "+7.1", "7.+1", "8.1", "7.0", "7", ".1", "7.1.1", "-0.1", "7.4294967297"] {
            assert!(t.get(bad).is_err(), "{bad}");
        }
        t.release(&a);
        let b = t.insert(7, None, "b").unwrap();
        assert_ne!(a, b, "a reused slot gets a new generation");
        assert!(t.get(&a).is_err());
        assert_eq!(t.get(&b).unwrap(), (7, &"b"));
        let c = t.insert(-3, None, "c").unwrap();
        assert_eq!(t.get(&c).unwrap(), (-3, &"c"));
        t.clear();
        assert!(t.get(&b).is_err() && t.get(&c).is_err());
        t.validate();
    }

    /// Random operations checked against a plain map and the recomputed
    /// context and group lists.
    #[test]
    fn handle_table_matches_a_model() {
        let mut t = HandleTable::new(HandleBudget::new(64));
        let mut model: HashMap<String, (ExecutionContextId, Option<String>, u32)> = HashMap::new();
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut rand = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        let mut ids: Vec<String> = Vec::new();
        for step in 0..20_000u32 {
            match rand(10) {
                0..=4 => {
                    let context = rand(4) as ExecutionContextId;
                    let group = match rand(4) {
                        0 => None,
                        g => Some(format!("g{g}")),
                    };
                    match t.insert(context, group.as_deref(), step) {
                        Ok(id) => {
                            assert!(model.insert(id.clone(), (context, group, step)).is_none(), "fresh id");
                            ids.push(id);
                        }
                        Err(_) => assert_eq!(model.len(), 64),
                    }
                }
                5 | 6 if !ids.is_empty() => {
                    let id = ids[rand(ids.len() as u64) as usize].clone();
                    assert_eq!(t.release(&id), model.remove(&id).map(|e| e.2));
                }
                7 => {
                    let group = format!("g{}", rand(4));
                    let before = model.len();
                    model.retain(|_, e| e.1.as_deref() != Some(&group));
                    assert_eq!(t.release_group(&group), before - model.len());
                }
                8 => {
                    let context = rand(4) as ExecutionContextId;
                    let before = model.len();
                    model.retain(|_, e| e.0 != context);
                    assert_eq!(t.release_context(context), before - model.len());
                }
                _ => {
                    if rand(50) == 0 {
                        t.clear();
                        model.clear();
                    }
                }
            }
            assert_eq!(t.len(), model.len());
            if step % 97 == 0 {
                t.validate();
                for id in &ids {
                    match model.get(id) {
                        Some((context, group, value)) => {
                            let f = t.lookup(id).unwrap();
                            assert_eq!(
                                (f.context, f.group.map(|g| &**g), *f.handle),
                                (*context, group.as_deref(), *value)
                            );
                        }
                        None => assert!(t.lookup(id).is_err()),
                    }
                }
            }
        }
        t.validate();
    }
}
