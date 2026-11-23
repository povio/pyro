//! WHATWG Streams. The classes are made in JS (streams.js) on a realm's first
//! use of any of them, so realms that never touch streams pay nothing beyond
//! one global accessor pair per interface name. The first read of a global name
//! replaces its accessor with a plain data property.
use std::rc::Rc;

use pyro_js::{ClassDef, JsClass, JsError, JsResult, Object, Scope, Value};

use crate::page::PageDom;
use crate::window::WindowHost;

/// Kept on the realm's `navigator` (a hidden property), itself kept per realm;
/// a worker's realm has the same names on its global.
const KEY: &str = "lpStreams";

/// Run in the engine's root context ([`Scope::agent_value`]): the internal
/// slots of the streams of every realm of the agent, one WeakMap per slot
/// name and origin. `slot(origin, name)` answers `{ has, get, set }` for
/// that map, functions of the root context over its own WeakMap and Map
/// primordials, which no page can reach or replace: page code never runs
/// with a map as receiver or argument, and only streams.js holds the
/// functions (they must never be handed to page code, whose realm could
/// then reach the root context's). Realms of one origin share the maps,
/// which is what lets a same-origin window's stream pass this realm's brand
/// checks; a realm of another origin, or of an opaque one (`origin` null:
/// maps of its own), shares nothing, so it can neither pass off nor touch
/// another origin's streams. Made there, the maps keep no page's realm alive.
const SLOTS: &str = r"(() => {
  'use strict';
  const apply = Reflect.apply;
  const freeze = Object.freeze;
  const WeakMapC = WeakMap;
  const weakGet = WeakMap.prototype.get;
  const weakSet = WeakMap.prototype.set;
  const weakHas = WeakMap.prototype.has;
  const mapGet = Map.prototype.get;
  const mapSet = Map.prototype.set;
  const maps = new Map();
  const isObject = (o) => (typeof o === 'object' && o !== null) || typeof o === 'function';
  return (origin, name) => {
    let map;
    if (typeof origin === 'string') {
      const key = origin + ' ' + name;
      map = apply(mapGet, maps, [key]);
      if (!map) {
        map = new WeakMapC();
        apply(mapSet, maps, [key, map]);
      }
    } else {
      map = new WeakMapC();
    }
    return freeze({
      __proto__: null,
      has: (o) => isObject(o) && apply(weakHas, map, [o]),
      get: (o) => (isObject(o) ? apply(weakGet, map, [o]) : undefined),
      set: (o, v) => {
        if (isObject(o)) apply(weakSet, map, [o, v]);
      },
    });
  };
})()";

/// A private slot shared by same-origin realms in this JS agent. An opaque
/// origin gets a fresh map, just as it does for the built-in stream slots.
pub(crate) fn shared_slot<'s>(s: &Scope<'s>, page: &PageDom, name: &str) -> JsResult<Value<'s>> {
    let slots = s.agent_value("streams.slot", SLOTS)?.as_object().ok_or_else(unavailable)?;
    let origin = crate::storage::document_origin(page).map_or_else(|| s.null(), |origin| s.string(&origin));
    slots.call(None, &[origin, s.string(name)])
}

macro_rules! lazy_globals {
    ($def:ident, $($name:literal),+ $(,)?) => {
        $($def.global_value_accessor(
            $name,
            |s, w| global(s, w.page(), $name),
            |s, w, value| replace(s, w.page(), $name, value),
        );)+
    };
}

/// Adds the lazy interface names to pyro-browser's `Window` class and to the
/// worker global scope.
pub(crate) fn define_globals<W>(def: &mut ClassDef<W>)
where
    W: JsClass,
    W::Data: WindowHost,
{
    lazy_globals!(
        def,
        "ReadableStream",
        "ReadableStreamDefaultReader",
        "ReadableStreamBYOBReader",
        "ReadableStreamDefaultController",
        "ReadableByteStreamController",
        "ReadableStreamBYOBRequest",
        "WritableStream",
        "WritableStreamDefaultWriter",
        "WritableStreamDefaultController",
        "TransformStream",
        "TransformStreamDefaultController",
        "ByteLengthQueuingStrategy",
        "CountQueuingStrategy",
        "TextEncoderStream",
        "TextDecoderStream",
        "CompressionStream",
        "DecompressionStream",
    );
}

fn global<'s>(s: &Scope<'s>, page: &Rc<PageDom>, name: &str) -> JsResult<Value<'s>> {
    let value = module(s, page)?.get(name)?;
    replace(s, page, name, value)?;
    Ok(value)
}

/// Shadows the accessor with the interface object's usual data property; also
/// what assigning the name does, so page polyfills keep working.
fn replace(s: &Scope<'_>, page: &Rc<PageDom>, name: &str, value: Value<'_>) -> JsResult<()> {
    let define = page.define_properties.borrow().get(&s.realm_id()).map(|p| p.get(s));
    let define = define.and_then(|v| v.as_object()).ok_or_else(unavailable)?;
    let desc = s.object();
    desc.set_atom("value", value)?;
    desc.set_atom("writable", s.boolean(true))?;
    desc.set_atom("enumerable", s.boolean(false))?;
    desc.set_atom("configurable", s.boolean(true))?;
    define.call(None, &[s.global().value(), s.string(name), desc.value()])?;
    Ok(())
}

fn unavailable() -> JsError {
    JsError::Type("streams are unavailable".into())
}

/// This realm's streams module, made on first use.
pub(crate) fn module<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Object<'s>> {
    let navigator = crate::worker_scope::module_holder(s, page)?;
    if let Some(module) = s.get_hidden(navigator, KEY)?.and_then(|v| v.as_object()) {
        return Ok(module);
    }
    let natives = s.object();
    natives.set_atom("zlib", crate::compression::zlib_function(s)?.value())?;
    natives.set_atom("slot", s.agent_value("streams.slot", SLOTS)?)?;
    let origin = match crate::storage::document_origin(page) {
        Some(origin) => s.string(&origin),
        None => s.null(),
    };
    natives.set_atom("origin", origin)?;
    natives.set_atom("fetchSettled", crate::fetch::settled_fetch_function(s)?)?;
    let natives = pyro_js::Persistent::new(s, natives.value());
    let factory = s.eval(include_str!("streams.js"), Some("pyro://internal/streams.js"), 1)?;
    let module = factory.as_object().ok_or_else(unavailable)?.call(None, &[natives.get(s)])?;
    s.set_hidden(navigator, KEY, module)?;
    module.as_object().ok_or_else(unavailable)
}

/// This realm's module when `value` may be a stream: the module when the
/// realm already made it, else made now only for an object that is a stream
/// of another realm of this origin (the slots are shared, see [`SLOTS`]), so
/// a realm that never used streams still tells such a stream apart, at the
/// cost of three lookups, without making its module for other values.
fn existing<'s>(s: &Scope<'s>, page: &Rc<PageDom>, value: Value<'s>) -> JsResult<Option<Object<'s>>> {
    let navigator = crate::worker_scope::module_holder(s, page)?;
    if let Some(module) = s.get_hidden(navigator, KEY)?.and_then(|v| v.as_object()) {
        return Ok(Some(module));
    }
    if value.as_object().is_none() {
        return Ok(None);
    }
    let slot = s.agent_value("streams.slot", SLOTS)?.as_object().ok_or_else(unavailable)?;
    let origin = match crate::storage::document_origin(page) {
        Some(origin) => s.string(&origin),
        None => return Ok(None),
    };
    for name in ["ReadableStream.s", "WritableStream.s", "TransformStream.s"] {
        let accessor = slot.call(None, &[origin, s.string(name)])?.as_object().ok_or_else(unavailable)?;
        let has = accessor.get_atom("has")?.as_object().ok_or_else(unavailable)?;
        if has.call(None, &[value])?.to_bool() {
            return module(s, page).map(Some);
        }
    }
    Ok(None)
}

/// Calls module helper `name`.
pub(crate) fn helper<'s>(s: &Scope<'s>, page: &Rc<PageDom>, name: &str, args: &[Value<'s>]) -> JsResult<Value<'s>> {
    let module = module(s, page)?;
    module.get(name)?.as_object().ok_or_else(unavailable)?.call(None, args)
}

/// `fetch(input, init)` when the body is a ReadableStream (`init.body`, or the
/// body of a Request made with one): the request goes out with the stream's
/// bytes once it ends (no streaming upload). None for any other call; realms
/// that never made their streams never read `init.body` here (a getter would
/// run twice), so they do not take a stream of another window as a streaming body.
pub(crate) fn fetch_stream_body<'s>(s: &Scope<'s>, input: Value<'s>, init: Value<'s>) -> JsResult<Option<Value<'s>>> {
    let page = crate::target::page_of(s)?;
    if existing(s, &page, s.undefined())?.is_none() {
        return Ok(None);
    }
    let body = match init.as_object() {
        Some(options) => options.get_atom("body")?,
        None => s.undefined(),
    };
    let stream = if is_stream(s, body)? {
        let options = init.as_object().expect("a body comes from an init object");
        if options.get_atom("duplex")?.as_string().as_deref() != Some("half") {
            return Err(JsError::Type(
                "The `duplex` member must be specified for a request with a streaming body".into(),
            ));
        }
        body
    } else if body.is_undefined() {
        match crate::request::script_body_stream(s, input)? {
            Some(stream) => stream,
            None => return Ok(None),
        }
    } else {
        return Ok(None);
    };
    // As new Request() (6334dbc3e): keepalive or no-cors with a stream body is a TypeError.
    let request = s.unwrap::<crate::request::Request>(input).map(|r| (r.keepalive, r.mode.clone(), r.url.clone()));
    let (mut keepalive, mut mode) = request.as_ref().map_or((false, "cors".to_owned()), |r| (r.0, r.1.clone()));
    if let Some(options) = init.as_object() {
        let value = options.get_atom("keepalive")?;
        if !value.is_undefined() {
            keepalive = value.to_bool();
        }
        let value = options.get_atom("mode")?;
        if !value.is_undefined() {
            mode = value.to_string()?;
        }
    }
    if keepalive {
        return Err(JsError::Type(
            "Failed to execute 'fetch' on 'Window': keepalive cannot be set for a request with a streaming body".into(),
        ));
    }
    if mode == "no-cors" {
        return Err(JsError::Type(
            "Failed to execute 'fetch' on 'Window': A no-cors request cannot have a streaming body".into(),
        ));
    }
    // Only an HTTP(S) fetch sends its body: a data:, blob: or about: one never reads the
    // stream (which may never end). An unparsable URL fails in `fetch` itself.
    let url = match request {
        Some((_, _, url)) => url,
        None => crate::util::dom_string(input)?,
    };
    let resolved = crate::html::resolve_url_utf8(&page.dom.borrow(), page.document(), &url);
    let network = pyro_net::Url::parse(&resolved).is_ok_and(|u| matches!(u.scheme(), "http" | "https"));
    helper(s, &page, "fetchBuffered", &[input, init, stream, s.boolean(network)]).map(Some)
}

/// Structured clone's transfer-list check: `"R"`, `"W"` or `"T"` when
/// `value` is a ReadableStream, WritableStream or TransformStream (of this
/// realm or another of its origin; DataCloneError when it is locked), else
/// `None`. A realm that never made its streams answers `None` for other
/// values at the cost of a few lookups ([`existing`]).
pub(crate) fn transfer_kind<'s>(s: &Scope<'s>, value: Value<'s>) -> JsResult<Option<Value<'s>>> {
    if value.as_object().is_none() {
        return Ok(None);
    }
    let page = crate::target::page_of(s)?;
    let Some(module) = existing(s, &page, value)? else { return Ok(None) };
    let kind = module.get_atom("transferKind")?.as_object().ok_or_else(unavailable)?.call(None, &[value])?;
    Ok((!kind.is_undefined()).then_some(kind))
}

/// The transfer steps of a stream [`transfer_kind`] accepted: the original
/// is piped through new MessageChannels; returns the ports to transfer.
pub(crate) fn transfer_out<'s>(s: &Scope<'s>, value: Value<'s>) -> JsResult<Object<'s>> {
    let page = crate::target::page_of(s)?;
    helper(s, &page, "transferOut", &[value])?.as_object().ok_or_else(unavailable)
}

/// The transfer-receiving steps: a stream of this realm of `kind` over the
/// MessagePorts in `ports` (this realm's).
pub(crate) fn transfer_in<'s>(s: &Scope<'s>, kind: Value<'s>, ports: Value<'s>) -> JsResult<Value<'s>> {
    let page = crate::target::page_of(s)?;
    helper(s, &page, "transferIn", &[kind, ports])
}

/// `value` is a ReadableStream (of this realm or another of its origin).
/// Cheap for primitives and for realms that have no streams.
pub(crate) fn is_stream<'s>(s: &Scope<'s>, value: Value<'s>) -> JsResult<bool> {
    if value.as_object().is_none() {
        return Ok(false);
    }
    let page = crate::target::page_of(s)?;
    let Some(module) = existing(s, &page, value)? else { return Ok(false) };
    Ok(module.get_atom("isStream")?.as_object().ok_or_else(unavailable)?.call(None, &[value])?.to_bool())
}
