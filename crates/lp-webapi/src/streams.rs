//! WHATWG Streams. The classes are made in JS (streams.js) on a realm's first
//! use of any of them, so realms that never touch streams pay nothing beyond
//! one global accessor pair per interface name. The first read of a global name
//! replaces its accessor with a plain data property.
use std::rc::Rc;

use lp_js::{ClassDef, JsClass, JsError, JsResult, Object, Scope, Value};

use crate::page::PageDom;
use crate::window::WindowHost;

/// Kept on the realm's `navigator` (a hidden property), itself kept per realm;
/// a worker's realm has the same names on its global.
const KEY: &str = "lpStreams";

macro_rules! lazy_globals {
    ($def:ident, $($name:literal),+ $(,)?) => {
        $($def.global_value_accessor(
            $name,
            |s, w| global(s, w.page(), $name),
            |s, w, value| replace(s, w.page(), $name, value),
        );)+
    };
}

/// Adds the lazy interface names to lp-browser's `Window` class and to the
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
    let natives = lp_js::Persistent::new(s, natives.value());
    let factory = s.eval(include_str!("streams.js"), Some("lightningpanda://internal/streams.js"), 1)?;
    let module = factory.as_object().ok_or_else(unavailable)?.call(None, &[natives.get(s)])?;
    s.set_hidden(navigator, KEY, module)?;
    module.as_object().ok_or_else(unavailable)
}

/// The module only when this realm already made it: without it, no value can
/// be a stream.
fn existing<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Option<Object<'s>>> {
    let navigator = crate::worker_scope::module_holder(s, page)?;
    Ok(s.get_hidden(navigator, KEY)?.and_then(|v| v.as_object()))
}

/// Calls module helper `name`.
pub(crate) fn helper<'s>(s: &Scope<'s>, page: &Rc<PageDom>, name: &str, args: &[Value<'s>]) -> JsResult<Value<'s>> {
    let module = module(s, page)?;
    module.get(name)?.as_object().ok_or_else(unavailable)?.call(None, args)
}

/// `fetch(input, init)` when `init.body` is a ReadableStream of this realm: the
/// request goes out with the stream's bytes once it ends (no streaming upload).
/// None for any other call; realms without streams never read `init.body` here.
pub(crate) fn fetch_stream_body<'s>(s: &Scope<'s>, input: Value<'s>, init: Value<'s>) -> JsResult<Option<Value<'s>>> {
    let Some(options) = init.as_object() else { return Ok(None) };
    let page = crate::target::page_of(s)?;
    if existing(s, &page)?.is_none() {
        return Ok(None);
    }
    let body = options.get_atom("body")?;
    if !is_stream(s, body)? {
        return Ok(None);
    }
    if options.get_atom("duplex")?.as_string().as_deref() != Some("half") {
        return Err(JsError::Type("The `duplex` member must be specified for a request with a streaming body".into()));
    }
    helper(s, &page, "fetchBuffered", &[input, init, body]).map(Some)
}

/// Structured clone's transfer-list check: `"R"`, `"W"` or `"T"` when
/// `value` is a ReadableStream, WritableStream or TransformStream of this
/// realm (DataCloneError when it is locked), else `None`. A realm that never
/// made its streams answers `None` at the cost of one lookup.
pub(crate) fn transfer_kind<'s>(s: &Scope<'s>, value: Value<'s>) -> JsResult<Option<Value<'s>>> {
    if value.as_object().is_none() {
        return Ok(None);
    }
    let page = crate::target::page_of(s)?;
    let Some(module) = existing(s, &page)? else { return Ok(None) };
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

/// `value` is a ReadableStream of this realm. Cheap for primitives and for
/// realms that have no streams.
pub(crate) fn is_stream<'s>(s: &Scope<'s>, value: Value<'s>) -> JsResult<bool> {
    if value.as_object().is_none() {
        return Ok(false);
    }
    let page = crate::target::page_of(s)?;
    let Some(module) = existing(s, &page)? else { return Ok(false) };
    Ok(module.get_atom("isStream")?.as_object().ok_or_else(unavailable)?.call(None, &[value])?.to_bool())
}
