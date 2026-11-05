//! WebIDL binding shapes the ecmascript-binding WPT suite checks: the
//! DOMException class, `[PutForwards]` setters, `[LegacyFactoryFunction]`s,
//! legacy callback interface objects and `sequence<T>` conversion.

use lp_js::*;
use std::cell::RefCell;

fn eval_str(realm: &Realm, src: &str) -> String {
    let s = realm.scope().unwrap();
    match s.eval(src, Some("test.js"), 1) {
        Ok(v) => v.to_string().unwrap(),
        Err(e) => format!("THROW {}", e.details(&s).text()),
    }
}

// -- classes under test -------------------------------------------------------

/// The forwarded-to object: `cssText`-like attribute `text`.
struct Style;
impl JsClass for Style {
    const NAME: &'static str = "FwdStyle";
    type Data = RefCell<String>;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.accessor(
            "text",
            |s, d| Ok(s.string(&d.borrow())),
            |_s, d, v| {
                *d.borrow_mut() = v.to_string()?;
                Ok(())
            },
        );
    }
}

/// Holds a `[PutForwards=text] style` attribute and a legacy factory.
struct Host;
impl JsClass for Host {
    const NAME: &'static str = "FwdHost";
    type Data = RefCell<Option<Persistent>>;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, target, _| s.wrap_new::<Self>(RefCell::new(None), target, None));
        def.put_forwards(
            "style",
            |s, d| {
                if let Some(p) = d.borrow().as_ref() {
                    return Ok(p.get(s));
                }
                let style = s.wrap::<Style>(RefCell::new(String::new()), None)?;
                *d.borrow_mut() = Some(Persistent::new(s, style.value()));
                Ok(style.value())
            },
            "text",
        );
        def.legacy_factory("MakeHost", 1, |s, target, args| {
            let host = s.wrap_new::<Self>(RefCell::new(None), target, None)?;
            host.set("tag", args.get(0))?;
            Ok(host)
        });
    }
}

/// A callback interface: constants only, no prototype.
struct Filter;
impl JsClass for Filter {
    const NAME: &'static str = "FwdFilter";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.legacy_callback_interface();
        def.constant("ACCEPT", 1.0);
    }
}

/// `takes(seq)` converts its argument as a `sequence<any>` and joins it.
struct Seq;
impl JsClass for Seq {
    const NAME: &'static str = "FwdSeq";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, target, _| s.wrap_new::<Self>((), target, None));
        def.method("takes", 1, |s, _d, args| {
            let list = s.sequence(args.get(0), "Failed to execute 'takes' on 'FwdSeq'")?;
            let n = list.get("length")?.to_number()? as u32;
            let mut out = Vec::new();
            for i in 0..n {
                out.push(list.get_index(i)?.to_string()?);
            }
            Ok(s.string(&out.join(",")))
        });
    }
}

fn realm() -> (Engine, Realm) {
    let engine = Engine::new().unwrap();
    engine.register::<Style>();
    engine.register::<Host>();
    engine.register::<Filter>();
    engine.register::<Seq>();
    let realm = engine.create_realm(Default::default()).unwrap();
    (engine, realm)
}

// -- DOMException ---------------------------------------------------------------

#[test]
fn dom_exception_members_are_inherited_brand_checked_accessors() {
    let (_e, r) = realm();
    assert_eq!(
        eval_str(
            &r,
            r#"(() => {
            const e = new DOMException("message", "NotFoundError");
            const own = ["message", "name", "code"].map((k) => e.hasOwnProperty(k));
            const desc = ["message", "name", "code"].map((k) => {
              const d = Object.getOwnPropertyDescriptor(DOMException.prototype, k);
              return typeof d.get === "function" && d.set === undefined && d.enumerable && d.configurable && d.get.name === "get " + k;
            });
            return [own.join(), desc.join(), e.message, e.name, e.code, e.toString(),
                    Object.prototype.toString.call(e), e instanceof Error, e instanceof DOMException].join("|");
          })()"#
        ),
        "false,false,false|true,true,true|message|NotFoundError|8|NotFoundError: message|[object DOMException]|true|true"
    );
    // Brand checks on every getter, the prototype included.
    for member in ["message", "name", "code"] {
        assert_eq!(
            eval_str(
                &r,
                &format!(
                    "try {{ Object.getOwnPropertyDescriptor(DOMException.prototype, '{member}').get.call({{}}); 'no' }} catch (e) {{ e.constructor.name }}"
                )
            ),
            "TypeError"
        );
    }
    assert_eq!(
        eval_str(&r, "try { DOMException.prototype.toString(); 'no' } catch (e) { e.constructor.name }"),
        "TypeError"
    );
}

#[test]
fn dom_exception_class_shape_and_construction() {
    let (_e, r) = realm();
    assert_eq!(
        eval_str(
            &r,
            r#"[
              Object.getPrototypeOf(DOMException) === Function.prototype,
              Object.getPrototypeOf(DOMException.prototype) === Error.prototype,
              DOMException.length, DOMException.name,
              (() => { try { DOMException(); return "no"; } catch (e) { return e.constructor.name; } })(),
              new DOMException().message === "" && new DOMException().name === "Error" && new DOMException().code === 0,
              new DOMException(undefined, undefined).name,
              new DOMException(null, "x").message,
              DOMException.INDEX_SIZE_ERR, DOMException.prototype.DATA_CLONE_ERR,
              Object.getOwnPropertyDescriptor(DOMException, "prototype").writable,
              typeof new DOMException("m").stack,
            ].join("|")"#
        ),
        "true|true|0|DOMException|TypeError|true|Error|null|1|25|false|string"
    );
    // Shadowing `name` on the instance does not change the code.
    assert_eq!(
        eval_str(
            &r,
            r#"(() => { const e = new DOMException("m", "InvalidCharacterError");
                 Object.defineProperty(e, "name", { value: "WrongDocumentError" });
                 Object.defineProperty(e, "message", { value: "new message" });
                 return e.code + "|" + e.toString(); })()"#
        ),
        "5|WrongDocumentError: new message"
    );
    // Subclassing keeps the brand and the subclass prototype.
    assert_eq!(
        eval_str(
            &r,
            r#"(() => { class Mine extends DOMException { constructor() { super("m", "AbortError"); } }
                 const e = new Mine();
                 return [Object.getPrototypeOf(e) === Mine.prototype, e.name, e.code, e instanceof DOMException].join(); })()"#
        ),
        "true,AbortError,20,true"
    );
    // The realm's QuotaExceededError still subclasses it.
    assert_eq!(
        eval_str(
            &r,
            r#"(() => { const e = new QuotaExceededError("full", { quota: 1, requested: 2 });
                 return [e instanceof DOMException, e.name, e.code, e.message, e.quota, e.requested, e.hasOwnProperty("message")].join(); })()"#
        ),
        "true,QuotaExceededError,22,full,1,2,false"
    );
}

#[test]
fn native_dom_errors_are_thrown_as_dom_exceptions() {
    let (_e, r) = realm();
    let s = r.scope().unwrap();
    let value = JsError::dom("HierarchyRequestError", "nope").to_value(&s);
    s.global().set("err", value).unwrap();
    assert_eq!(
        eval_str(
            &r,
            r#"[err instanceof DOMException, err.name, err.code, err.message, err.hasOwnProperty("message"),
                Object.getPrototypeOf(err) === DOMException.prototype].join()"#
        ),
        "true,HierarchyRequestError,3,nope,false,true"
    );
}

// -- [PutForwards] ----------------------------------------------------------------

#[test]
fn put_forwards_setter_gets_the_attribute_then_sets_the_target() {
    let (_e, r) = realm();
    assert_eq!(
        eval_str(
            &r,
            r#"(() => {
              const h = new FwdHost();
              const d = Object.getOwnPropertyDescriptor(FwdHost.prototype, "style");
              h.style = "red";
              const plain = h.style.text;
              // Overridden getter on the instance is what the setter reads (once per assignment).
              let got = 0; const real = h.style;
              Object.defineProperty(h, "style", { get() { got++; return real; }, set: d.set, configurable: true });
              h.style = "green";
              // Overridden setter of the target attribute runs.
              let setCalled = false;
              const td = Object.getOwnPropertyDescriptor(FwdStyle.prototype, "text");
              Object.defineProperty(real, "text", { get: td.get, set(v) { setCalled = true; td.set.call(this, v); } });
              h.style = "blue";
              // A different target object is updated instead.
              const fake = { text: "orig" };
              Object.defineProperty(h, "style", { get() { return fake; }, set: d.set, configurable: true });
              h.style = "pink";
              return [plain, got, real.text, setCalled, fake.text, d.set.name, d.set.length].join("|");
            })()"#
        ),
        "red|2|blue|true|pink|set style|1"
    );
}

#[test]
fn put_forwards_setter_errors() {
    let (_e, r) = realm();
    let kind = |src: &str| {
        eval_str(
            &r,
            &format!(
                r#"(() => {{ const h = new FwdHost(); const d = Object.getOwnPropertyDescriptor(FwdHost.prototype, "style");
                   try {{ {src}; return "no"; }} catch (e) {{ return e.constructor.name; }} }})()"#
            ),
        )
    };
    // Invalid receivers, with or without an argument.
    assert_eq!(kind("d.set.call({})"), "TypeError");
    assert_eq!(kind("d.set.call({}, 'x')"), "TypeError");
    assert_eq!(kind("d.set.call(1, 'x')"), "TypeError");
    // Getter exceptions propagate; a non-object attribute value is a TypeError.
    assert_eq!(
        kind(r#"Object.defineProperty(h, "style", { get() { throw new SyntaxError(); }, set: d.set }); h.style = "x""#),
        "SyntaxError"
    );
    assert_eq!(
        kind(r#"Object.defineProperty(h, "style", { get() { return null; }, set: d.set }); h.style = "x""#),
        "TypeError"
    );
    // Target setter exceptions propagate.
    assert_eq!(
        kind(r#"Object.defineProperty(h.style, "text", { set() { throw new RangeError(); } }); h.style = "x""#),
        "RangeError"
    );
    // A non-writable target just fails silently (Set with Throw = false).
    assert_eq!(
        eval_str(
            &r,
            r#"(() => { const h = new FwdHost(); const st = h.style;
                 Object.defineProperty(st, "text", { value: null, writable: false });
                 h.style = "x"; return h.style === st && st.text === null; })()"#
        ),
        "true"
    );
    // No argument means undefined.
    assert_eq!(
        eval_str(
            &r,
            r#"(() => { const h = new FwdHost(); Object.getOwnPropertyDescriptor(FwdHost.prototype, "style").set.call(h); return h.style.text; })()"#
        ),
        "undefined"
    );
}

// -- [LegacyFactoryFunction] -----------------------------------------------------

#[test]
fn legacy_factory_function_shape_and_new_target() {
    let (_e, r) = realm();
    assert_eq!(
        eval_str(
            &r,
            r#"(() => {
              const d = Object.getOwnPropertyDescriptor(globalThis, "MakeHost");
              const pd = Object.getOwnPropertyDescriptor(MakeHost, "prototype");
              const h = new MakeHost("t");
              class Custom extends MakeHost { constructor() { super("c"); } }
              const c = new Custom();
              return [
                Reflect.ownKeys(MakeHost).slice(0, 3).join(),
                d.writable && !d.enumerable && d.configurable,
                MakeHost.length, MakeHost.name, typeof MakeHost,
                pd.value === FwdHost.prototype && !pd.writable && !pd.enumerable && !pd.configurable,
                Object.getPrototypeOf(MakeHost) === Function.prototype,
                Object.getPrototypeOf(h) === FwdHost.prototype, h instanceof FwdHost, h.tag,
                Object.getPrototypeOf(c) === Custom.prototype, c instanceof Custom, c instanceof FwdHost, c.tag,
                (() => { try { MakeHost(); return "no"; } catch (e) { return e.constructor.name; } })(),
              ].join("|");
            })()"#
        ),
        "length,name,prototype|true|1|MakeHost|function|true|true|true|true|t|true|true|true|c|TypeError"
    );
}

#[test]
fn legacy_factory_populates_the_interface_members_lazily() {
    let (engine, r) = realm();
    // Nothing of FwdHost was touched before: the factory's prototype still
    // has the members (installed on first use through the factory).
    assert_eq!(eval_str(&r, "typeof Object.getOwnPropertyDescriptor(MakeHost.prototype, 'style').get"), "function");
    let second = engine.create_realm(Default::default()).unwrap();
    assert_eq!(eval_str(&second, "Reflect.ownKeys(MakeHost).join()"), "length,name,prototype");
    assert_eq!(eval_str(&second, "'style' in new MakeHost()"), "true");
}

// -- legacy callback interface ---------------------------------------------------

#[test]
fn legacy_callback_interface_object_has_no_prototype() {
    let (_e, r) = realm();
    assert_eq!(
        eval_str(
            &r,
            r#"[
              typeof FwdFilter, Object.getPrototypeOf(FwdFilter) === Function.prototype,
              FwdFilter.hasOwnProperty("prototype"), FwdFilter.ACCEPT, FwdFilter.name, FwdFilter.length,
              (() => { try { FwdFilter(); return "no"; } catch (e) { return e.constructor.name; } })(),
              (() => { try { new FwdFilter(); return "no"; } catch (e) { return e.constructor.name; } })(),
              5 instanceof FwdFilter,
              (() => { try { ({}) instanceof FwdFilter; return "no"; } catch (e) { return e.constructor.name; } })(),
              (() => { try { (function () {}) instanceof FwdFilter; return "no"; } catch (e) { return e.constructor.name; } })(),
            ].join("|")"#
        ),
        "function|true|false|1|FwdFilter|0|TypeError|TypeError|false|TypeError|TypeError"
    );
}

// -- sequence<T> ------------------------------------------------------------------

#[test]
fn sequence_conversion_uses_the_iterator_protocol() {
    let (_e, r) = realm();
    assert_eq!(eval_str(&r, "new FwdSeq().takes([1, 2])"), "1,2");
    assert_eq!(eval_str(&r, "new FwdSeq().takes((function* () { yield 4; yield 5; })())"), "4,5");
    assert_eq!(
        eval_str(
            &r,
            r#"(() => { const a = [1, 2]; let got = 0;
                 Object.defineProperty(a, Symbol.iterator, { get() { got++; return function* () { yield 6; yield 7; }; } });
                 return new FwdSeq().takes(a) + "|" + got; })()"#
        ),
        "6,7|1"
    );
    assert_eq!(
        eval_str(
            &r,
            r#"(() => { Object.defineProperty(Array.prototype, "1", { configurable: true, get() { return 14; } });
                 try { return new FwdSeq().takes([13, , 15]); } finally { delete Array.prototype[1]; } })()"#
        ),
        "13,14,15"
    );
    assert_eq!(
        eval_str(
            &r,
            r#"(() => { const re = /x/; re[Symbol.iterator] = function* () { yield "p"; }; return new FwdSeq().takes(re); })()"#
        ),
        "p"
    );
    for bad in ["1", "null", "undefined", "{}", "'ab'"] {
        assert_eq!(
            eval_str(&r, &format!("try {{ new FwdSeq().takes({bad}); 'no' }} catch (e) {{ e.constructor.name }}")),
            "TypeError",
            "{bad}"
        );
    }
}

// -- review additions ---------------------------------------------------------------

#[test]
fn dom_exception_converts_arguments_as_dom_strings_then_reads_new_target_prototype() {
    let (_e, r) = realm();
    // DOMString conversion is ToString: Symbols throw instead of being described.
    for src in ["new DOMException(Symbol('m'))", "new DOMException('m', Symbol('n'))"] {
        assert_eq!(
            eval_str(&r, &format!("try {{ {src}; 'no' }} catch (e) {{ e.constructor.name }}")),
            "TypeError",
            "{src}"
        );
    }
    // A NewTarget without an object `prototype` falls back to DOMException.prototype
    // (not Error.prototype), and the instance keeps its brand.
    assert_eq!(
        eval_str(
            &r,
            r#"(() => { function NT() {} NT.prototype = null;
                 const e = Reflect.construct(DOMException, ["m", "AbortError"], NT);
                 NT.prototype = 5;
                 const f = Reflect.construct(DOMException, [], NT);
                 return [Object.getPrototypeOf(e) === DOMException.prototype, e.name, e.code, e.message,
                         Object.getPrototypeOf(f) === DOMException.prototype].join(); })()"#
        ),
        "true,AbortError,20,m,true"
    );
    // The prototype used is the one read after both arguments are converted.
    assert_eq!(
        eval_str(
            &r,
            r#"(() => { const log = [];
                 const proto = Object.create(DOMException.prototype);
                 const NT = new Proxy(function () {}, { get(t, k) { if (k === "prototype") { log.push("prototype"); return proto; } return Reflect.get(t, k); } });
                 const e = Reflect.construct(DOMException, [{ toString() { log.push("message"); return "m"; } },
                                                           { toString() { log.push("name"); return "n"; } }], NT);
                 return log.slice(-3).join() + "|" + (Object.getPrototypeOf(e) === proto) + "|" + e.message + e.name; })()"#
        ),
        "message,name,prototype|true|mn"
    );
}

#[test]
fn error_previews_read_dom_exception_messages_without_running_getters() {
    let (_e, r) = realm();
    let s = r.scope().unwrap();
    let preview = |src: &str| {
        let v = s.eval(src, Some("test.js"), 1).unwrap();
        s.object_preview(v, None, None).unwrap()
    };
    // The inherited accessor's value is in the preview, like an own message.
    let dom = preview("new DOMException('boom', 'AbortError')");
    assert!(dom.contains(r#""name":"message""#) && dom.contains(r#""value":"boom""#), "{dom}");
    let own = preview("new Error('plain')");
    assert!(own.contains(r#""name":"message""#) && own.contains(r#""value":"plain""#), "{own}");
    // An error without an own message has none in the preview, and a
    // page-defined `message` getter on its prototype chain is never run.
    let bare = preview("new Error()");
    assert!(!bare.contains(r#""name":"message""#), "{bare}");
    let custom = preview(
        "globalThis.ran = 0; class Custom extends Error { get message() { globalThis.ran++; return 'side effect'; } } new Custom()",
    );
    assert!(!custom.contains("side effect"), "{custom}");
    assert_eq!(s.eval("ran", None, 1).unwrap().to_string().unwrap(), "0");
}

#[test]
fn put_forwards_setters_work_in_every_engine() {
    // The setter's identifier pair is interned per process, not per engine.
    for _ in 0..3 {
        let (_e, r) = realm();
        assert_eq!(eval_str(&r, "(() => { const h = new FwdHost(); h.style = 'x'; return h.style.text; })()"), "x");
    }
}

#[test]
fn sequence_conversion_ignores_a_replaced_symbol_global() {
    let (_e, r) = realm();
    assert_eq!(
        eval_str(
            &r,
            r#"(() => { const S = globalThis.Symbol;
                 try { globalThis.Symbol = { iterator: "nope" }; return new FwdSeq().takes([1, 2]); }
                 finally { globalThis.Symbol = S; } })()"#
        ),
        "1,2"
    );
}

// -- named properties object ---------------------------------------------------------

struct Target;
impl JsClass for Target {
    const NAME: &'static str = "EventTarget";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("listen", 0, |s, _d, _a| Ok(s.string("listening")));
    }
}

struct Window;
impl JsClass for Window {
    const NAME: &'static str = "Window";
    type Data = ();
    type Parent = Target;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("document", |s, _d| Ok(s.string("the document")));
    }
}

/// Every name in `NAMES` resolves to `named:<name>`.
struct Names;
const NAMES: [&str; 6] = ["plain", "document", "listen", "toString", "shadowed", "0"];
impl NamedProperties for Names {
    fn may_resolve(&self, name: NameUnits<'_>) -> bool {
        let name = match name {
            NameUnits::Latin1(n) => String::from_utf8_lossy(n).into_owned(),
            NameUnits::Utf16(n) => String::from_utf16_lossy(n),
        };
        NAMES.contains(&name.as_str())
    }
    fn resolve<'s>(&self, s: &Scope<'s>, name: &str) -> JsResult<Option<Value<'s>>> {
        Ok(NAMES.contains(&name).then(|| s.string(&format!("named:{name}"))))
    }
}

#[test]
fn named_properties_object_sits_between_the_global_interface_and_its_parent() {
    let engine = Engine::new().unwrap();
    engine.register::<Target>();
    let window = engine.register::<Window>();
    let r = engine.create_realm(RealmOptions { global_class: Some(window), ..Default::default() }).unwrap();
    r.set_global_data::<Window>(()).unwrap();
    r.set_named_properties(std::rc::Rc::new(Names));
    // Only a JavaScriptCore with the named-properties SPI has the object.
    if eval_str(&r, "Object.prototype.toString.call(Object.getPrototypeOf(globalThis))") != "[object WindowProperties]"
    {
        return;
    }
    let wp = "const wp = Object.getPrototypeOf(Window.prototype);";
    // The object and its internal methods.
    assert_eq!(
        eval_str(
            &r,
            &format!(
                r#"(() => {{ {wp}
                 return [
                   Object.prototype.toString.call(wp), Object.getPrototypeOf(wp) === EventTarget.prototype,
                   Reflect.ownKeys(wp).length === 1 && Reflect.ownKeys(wp)[0] === Symbol.toStringTag,
                   Reflect.preventExtensions(wp), Object.isExtensible(wp), Reflect.defineProperty(wp, "x", {{}}),
                   Reflect.deleteProperty(wp, Symbol.toStringTag), Reflect.deleteProperty(wp, "plain"),
                   Reflect.setPrototypeOf(wp, EventTarget.prototype), Reflect.setPrototypeOf(wp, {{}}),
                   (() => {{ try {{ wp.__proto__ = null; return "no"; }} catch (e) {{ return e.constructor.name; }} }})(),
                 ].join("|"); }})()"#
            )
        ),
        "[object WindowProperties]|true|true|false|true|false|false|false|true|false|TypeError"
    );
    // A supported name is an own, writable, non-enumerable, configurable data property.
    assert_eq!(
        eval_str(
            &r,
            &format!(
                r#"(() => {{ {wp} const d = Object.getOwnPropertyDescriptor(wp, "plain");
                 return [wp.plain, "plain" in wp, d.value, d.writable, d.enumerable, d.configurable,
                         wp[0], String(wp.missing), "missing" in wp, Object.create(Window.prototype).plain].join("|"); }})()"#
            )
        ),
        "named:plain|true|named:plain|true|false|true|named:0|undefined|false|named:plain"
    );
    // Named property visibility: hidden by an own property of the global
    // (interface members and page globals), of the interface prototype
    // object, and of the objects further up the chain.
    assert_eq!(
        eval_str(
            &r,
            &format!(
                r#"(() => {{ {wp}
                 const before = wp.shadowed;
                 globalThis.shadowed = 1;
                 const globalOwn = [String(wp.shadowed), "shadowed" in wp, String(Object.getOwnPropertyDescriptor(wp, "shadowed"))].join();
                 delete globalThis.shadowed;
                 Window.prototype.shadowed = 2;
                 const protoOwn = [String(Object.getOwnPropertyDescriptor(wp, "shadowed")), Object.create(Window.prototype).shadowed].join();
                 delete Window.prototype.shadowed;
                 return [before, globalOwn, protoOwn, wp.shadowed,
                         String(wp.document), String(Object.getOwnPropertyDescriptor(wp, "document")),
                         wp.listen === EventTarget.prototype.listen, String(Object.getOwnPropertyDescriptor(wp, "listen")),
                         wp.toString === Object.prototype.toString].join("|"); }})()"#
            )
        ),
        "named:shadowed|undefined,false,undefined|undefined,2|named:shadowed|undefined|undefined|true|undefined|true"
    );
    // [[Set]]: fails with the object as receiver, named or not; a setter
    // further up the chain runs with the receiver; other receivers get an
    // own property.
    assert_eq!(
        eval_str(
            &r,
            &format!(
                r#"(() => {{ {wp}
                 let seen;
                 Object.defineProperty(Object.prototype, "viaSetter", {{ configurable: true, set(v) {{ seen = this; }} }});
                 const viaSetter = Reflect.set(wp, "viaSetter", 1) && seen === wp;
                 delete Object.prototype.viaSetter;
                 const receiver = Object.create(wp);
                 receiver.plain = 10;
                 const other = {{}};
                 return [Reflect.set(wp, "plain", 1), Reflect.set(wp, "missing", 1), Reflect.set(wp, Symbol.toStringTag, "x"),
                         (() => {{ "use strict"; try {{ wp.plain = 1; return "no"; }} catch (e) {{ return e.constructor.name; }} }})(),
                         wp.plain, viaSetter, receiver.plain, Object.hasOwn(receiver, "plain"),
                         Reflect.set(wp, "plain", 5, other), other.plain].join("|"); }})()"#
            )
        ),
        "false|false|false|TypeError|named:plain|true|10|true|true|5"
    );
}
