//! WebIDL binding behaviours of the DOM classes as the ecmascript-binding WPT
//! suite sees them: `[PutForwards]` on `style` and `relList`, the `Image`,
//! `Option` and `Audio` legacy factory functions, the `NodeFilter` callback
//! interface object and DOMExceptions thrown by DOM operations.

use crate::support::network::Env;

fn env() -> Env {
    Env::new("http://webidl.test/")
}

#[test]
fn style_assignment_put_forwards_to_css_text() {
    let env = env();
    assert_eq!(
        env.eval(
            r#"(() => {
              const d = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "style");
              const out = [];
              // Plain assignment goes through cssText.
              let el = document.createElement("div");
              el.style = "color: green";
              out.push(el.style.color + "/" + el.getAttribute("style"));
              // The overridden getter on the instance is read.
              el = document.createElement("div");
              const real = el.style;
              let got = false;
              Object.defineProperty(el, "style", { get() { got = true; return real; }, set: d.set });
              el.style = "color: green";
              out.push(got + "/" + real.color);
              // The overridden cssText setter on the declaration runs.
              el = document.createElement("div");
              const cd = Object.getOwnPropertyDescriptor(CSSStyleDeclaration.prototype, "cssText");
              let setCalled = false;
              Object.defineProperty(el.style, "cssText", { get: cd.get, set(v) { setCalled = true; cd.set.call(this, v); } });
              el.style = "color: green";
              out.push(setCalled + "/" + el.style.color);
              // A different getter result is the one updated.
              el = document.createElement("div");
              const original = el.style;
              const fake = { cssText: "original" };
              Object.defineProperty(el, "style", { get() { return fake; }, set: d.set });
              el.style = "color: green";
              out.push(original.cssText + "/" + fake.cssText);
              // A non-writable cssText fails silently.
              el = document.createElement("div");
              const st = el.style;
              Object.defineProperty(st, "cssText", { value: null, writable: false });
              el.style = "color: green";
              out.push((el.style === st) + "/" + st.cssText);
              return out.join("|");
            })()"#
        ),
        "green/color: green;|true/green|true/green|/color: green|true/null"
    );
    let kind = |src: &str| {
        env.eval(&format!(
            r#"(() => {{ const el = document.createElement("div"); const d = Object.getOwnPropertyDescriptor(HTMLElement.prototype, "style");
                try {{ {src}; return "no"; }} catch (e) {{ return e.constructor.name; }} }})()"#
        ))
    };
    assert_eq!(
        kind(
            r#"Object.defineProperty(el, "style", { get() { throw new SyntaxError(); }, set: d.set }); el.style = "x""#
        ),
        "SyntaxError"
    );
    assert_eq!(
        kind(r#"Object.defineProperty(el.style, "cssText", { set() { throw new SyntaxError(); } }); el.style = "x""#),
        "SyntaxError"
    );
    assert_eq!(
        kind(r#"Object.defineProperty(el, "style", { get() { return null; }, set: d.set }); el.style = "x""#),
        "TypeError"
    );
    for this in ["undefined", "null", "1", "{}", "document"] {
        assert_eq!(kind(&format!("d.set.call({this}, 'x')")), "TypeError", "{this}");
        assert_eq!(kind(&format!("d.set.call({this})")), "TypeError", "{this}");
    }
    // SVG elements forward the same way.
    assert_eq!(
        env.eval(r#"(() => { const el = document.createElementNS("http://www.w3.org/2000/svg", "svg"); el.style = "fill: red"; return el.getAttribute("style"); })()"#),
        "fill: red;"
    );
}

#[test]
fn rel_list_assignment_put_forwards_to_value() {
    let env = env();
    for (tag, value) in [("a", "noreferrer"), ("area", "noreferrer"), ("form", "noreferrer"), ("link", "stylesheet")] {
        assert_eq!(
            env.eval(&format!(
                r#"(() => {{ const el = document.createElement("{tag}");
                     const list = el.relList instanceof DOMTokenList;
                     el.relList = "{value}"; const set = el.getAttribute("rel");
                     el.relList = ""; const cleared = el.getAttribute("rel");
                     return [list, set, cleared, el.rel].join("|"); }})()"#
            )),
            format!("true|{value}||"),
            "{tag}"
        );
    }
    // The setter with no argument stringifies undefined, through the list's own setter.
    assert_eq!(
        env.eval(
            r#"(() => { const a = document.createElement("a");
                 const setter = Object.getOwnPropertyDescriptor(HTMLAnchorElement.prototype, "relList").set;
                 const before = a.relList.length;
                 setter.call(a);
                 let overridden = null;
                 const vd = Object.getOwnPropertyDescriptor(DOMTokenList.prototype, "value");
                 Object.defineProperty(a.relList, "value", { get: vd.get, set(v) { overridden = v; } });
                 a.relList = "x y";
                 return [before, a.relList.length, a.relList[0], overridden, a.getAttribute("rel")].join("|"); })()"#
        ),
        "0|1|undefined|x y|undefined"
    );
}

#[test]
fn legacy_factory_functions_image_option_audio() {
    let env = env();
    assert_eq!(
        env.eval(
            r#"(() => {
              const i = new Image(), j = new Image(10, 20);
              const o = new Option(), p = new Option("text", "val", true, true);
              const a = new Audio(), b = new Audio("song.mp3");
              return [
                Reflect.ownKeys(Image).slice(0, 3).join(), Reflect.ownKeys(Option).slice(0, 3).join(),
                Image.length, Option.length, Audio.length, Image.name, Audio.name,
                Image.prototype === HTMLImageElement.prototype, Option.prototype === HTMLOptionElement.prototype,
                Audio.prototype === HTMLAudioElement.prototype,
                i.localName, i.ownerDocument === document, i.hasAttribute("width"), i instanceof HTMLImageElement,
                j.getAttribute("width") + "x" + j.getAttribute("height"), j.width,
                o.localName, o.text, o.hasAttribute("value"), o.selected,
                p.text, p.value, p.defaultSelected, p.selected, p.hasAttribute("selected"),
                a.localName, a.getAttribute("preload"), a.hasAttribute("src"), b.getAttribute("src"), b instanceof HTMLAudioElement,
                i.constructor === HTMLImageElement, Object.getPrototypeOf(Image) === Function.prototype,
                (() => { try { Image(); return "no"; } catch (e) { return e.constructor.name; } })(),
              ].join("|");
            })()"#
        ),
        "length,name,prototype|length,name,prototype|0|0|0|Image|Audio|true|true|true|img|true|false|true|10x20|10|option||false|false|text|val|true|true|true|audio|auto|false|song.mp3|true|true|true|TypeError"
    );
    // Subclassing honours NewTarget.
    assert_eq!(
        env.eval(
            r#"(() => { class CustomImage extends Image {} const x = new CustomImage();
                 return [Object.getPrototypeOf(x) === CustomImage.prototype, x instanceof CustomImage, x instanceof HTMLImageElement, x.localName].join(); })()"#
        ),
        "true,true,true,img"
    );
}

#[test]
fn legacy_factory_arguments_are_converted_before_the_dom_is_borrowed() {
    let env = env();
    // `toString`/`valueOf` of an argument is page script and may touch the DOM.
    assert_eq!(
        env.eval(
            r#"(() => {
              const touch = (v) => ({ toString() { document.documentElement.setAttribute("touched", v); return v; },
                                      valueOf() { document.documentElement.setAttribute("touched", "n"); return 7; } });
              const a = new Audio(touch("song.mp3"));
              const o = new Option(touch("text"), touch("val"));
              const i = new Image(touch("w"), touch("h"));
              return [a.getAttribute("src"), o.text, o.value, i.width, i.height, document.documentElement.getAttribute("touched")].join("|");
            })()"#
        ),
        "song.mp3|text|val|7|7|n"
    );
}

#[test]
fn option_factory_sets_selectedness_without_dirtiness() {
    let env = env();
    // HTML's Option constructor sets the selectedness only: an option whose
    // selectedness matches its `selected` attribute keeps following it.
    assert_eq!(
        env.eval(
            r#"(() => {
              const clean = new Option("a", "b"); clean.defaultSelected = true;
              const both = new Option("a", "b", true, true); both.defaultSelected = false;
              const unselected = new Option("a", "b", true, false);
              const selected = new Option("a", "b", false, true);
              const select = document.createElement("select");
              select.add(new Option("x", "1")); select.add(new Option("y", "2", true, true)); select.add(new Option("z", "3"));
              return [clean.selected, both.selected, unselected.selected, unselected.defaultSelected,
                      selected.selected, selected.defaultSelected, select.value, select.selectedIndex].join("|");
            })()"#
        ),
        "true|false|false|true|true|false|2|1"
    );
}

#[test]
fn node_filter_is_a_legacy_callback_interface_object() {
    let env = env();
    assert_eq!(
        env.eval(
            r#"[
              typeof NodeFilter, Object.getPrototypeOf(NodeFilter) === Function.prototype,
              NodeFilter.hasOwnProperty("prototype"), NodeFilter.FILTER_ACCEPT, NodeFilter.SHOW_ELEMENT, NodeFilter.name, NodeFilter.length,
              (() => { try { NodeFilter(); return "no"; } catch (e) { return e.constructor.name; } })(),
              (() => { try { new NodeFilter(); return "no"; } catch (e) { return e.constructor.name; } })(),
              5 instanceof NodeFilter,
              (() => { try { ({}) instanceof NodeFilter; return "no"; } catch (e) { return e.constructor.name; } })(),
              // Walkers still use the constants.
              document.createTreeWalker(document, NodeFilter.SHOW_ELEMENT, { acceptNode: () => NodeFilter.FILTER_ACCEPT }).nextNode().localName,
            ].join("|")"#
        ),
        "function|true|false|1|1|NodeFilter|0|TypeError|TypeError|false|TypeError|html"
    );
}

#[test]
fn dom_exceptions_from_dom_operations() {
    let env = env();
    assert_eq!(
        env.eval(
            r#"(() => { let e = null; try { document.documentElement.appendChild(document); } catch (x) { e = x; }
                 return [Object.getPrototypeOf(e) === DOMException.prototype, e.hasOwnProperty("name"), e.hasOwnProperty("message"),
                         e.name, e.code === DOMException.HIERARCHY_REQUEST_ERR, Object.prototype.toString.call(e), typeof e.stack].join("|"); })()"#
        ),
        "true|false|false|HierarchyRequestError|true|[object DOMException]|string"
    );
}

#[test]
fn window_prototype_chains_through_the_named_properties_object() {
    let env = env();
    // Only a global created with JSC's named-properties support has the
    // WindowProperties object; the chain is Window.prototype -> it -> EventTarget.prototype.
    let result = env.eval(
        r#"(() => {
              const real = Object.getPrototypeOf(globalThis);
              const tag = real && Object.getOwnPropertyDescriptor(real, Symbol.toStringTag);
              if (!tag || tag.value !== "WindowProperties") return "n/a";
              const wp = Object.getPrototypeOf(Window.prototype);
              return [
                Object.prototype.toString.call(wp), Object.getPrototypeOf(wp) === EventTarget.prototype,
                Reflect.ownKeys(wp).length === 1 && Reflect.ownKeys(wp)[0] === Symbol.toStringTag,
                Reflect.preventExtensions(wp), Object.isExtensible(wp), Reflect.defineProperty(wp, "x", {}),
                Reflect.set(wp, "x", 1), Reflect.deleteProperty(wp, Symbol.toStringTag),
                (() => { try { wp.__proto__ = null; return "no"; } catch (e) { return e.constructor.name; } })(),
                Reflect.setPrototypeOf(wp, EventTarget.prototype), Reflect.setPrototypeOf(wp, {}),
              ].join("|");
            })()"#,
    );
    assert!(
        result == "n/a"
            || result == "[object WindowProperties]|true|true|false|true|false|false|false|TypeError|true|false",
        "{result}"
    );
}

