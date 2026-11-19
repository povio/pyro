//! SVG elements' enumerated attributes (`clipPathUnits`, `maskUnits`,
//! `spreadMethod`, ...) as `SVGAnimatedEnumeration`s: the same object each
//! time, `baseVal` and `animVal` read the attribute, and setting `baseVal`
//! writes its keyword (0 and values without one throw a TypeError).

use crate::support::network::Env;

#[test]
fn units_and_spread_method_reflect_as_animated_enumerations() {
    let env = Env::new("http://svg.test/");
    assert_eq!(
        env.eval(
            r#"(() => {
              const ns = "http://www.w3.org/2000/svg";
              const clip = document.createElementNS(ns, "clipPath");
              const out = [clip instanceof SVGClipPathElement, clip.clipPathUnits === clip.clipPathUnits, clip.clipPathUnits.baseVal];
              clip.setAttribute("clipPathUnits", "objectBoundingBox");
              out.push(clip.clipPathUnits.baseVal, clip.clipPathUnits.animVal);
              clip.clipPathUnits.baseVal = SVGClipPathElement.SVG_UNIT_TYPE_USERSPACEONUSE;
              out.push(clip.getAttribute("clipPathUnits"));
              try { clip.clipPathUnits.baseVal = 0; out.push("no throw"); } catch (e) { out.push(e.constructor.name); }
              const mask = document.createElementNS(ns, "mask");
              out.push(mask.maskUnits.baseVal, mask.maskContentUnits.baseVal);
              const gradient = document.createElementNS(ns, "linearGradient");
              out.push(gradient instanceof SVGGradientElement, gradient.spreadMethod.baseVal);
              gradient.spreadMethod.baseVal = SVGGradientElement.SVG_SPREADMETHOD_REPEAT;
              out.push(gradient.getAttribute("spreadMethod"));
              out.push(document.createElementNS(ns, "marker").markerUnits.baseVal);
              return out.join("|");
            })()"#
        ),
        "true|true|1|2|2|userSpaceOnUse|TypeError|2|1|true|1|repeat|2"
    );
}

/// `SVGSVGElement` time control of the document's SMIL clock: `setCurrentTime` (finite only),
/// `getCurrentTime`, `pauseAnimations`, `unpauseAnimations`, `animationsPaused` (WPT
/// svg/animations end-of-time crashtests seek to huge times).
#[test]
fn svg_time_control_seeks_and_pauses() {
    let env = Env::new("http://svg.test/");
    assert_eq!(
        env.eval(
            r#"(() => {
              const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
              const out = [svg.animationsPaused()];
              svg.pauseAnimations();
              svg.setCurrentTime(5);
              out.push(svg.animationsPaused(), svg.getCurrentTime());
              svg.setCurrentTime(18446744073709551557);
              out.push(svg.getCurrentTime() > 1e18);
              try { svg.setCurrentTime(NaN); out.push("no throw"); } catch (e) { out.push(e.constructor.name); }
              svg.unpauseAnimations();
              out.push(svg.animationsPaused());
              return out.join("|");
            })()"#
        ),
        "false|true|5|true|TypeError|false"
    );
}

/// MathML elements are `MathMLElement`s with an inline `style` and `dataset` (WPT mathml
/// dynamic-height-within-table-cell sets `style.height` on an `<mrow>`).
#[test]
fn mathml_elements_have_inline_style() {
    let env = Env::new("http://svg.test/");
    assert_eq!(
        env.eval(
            r#"(() => {
              const mrow = document.createElementNS("http://www.w3.org/1998/Math/MathML", "mrow");
              mrow.style.height = "5px";
              return [mrow instanceof MathMLElement, mrow instanceof Element, mrow.getAttribute("style"), typeof mrow.dataset].join("|");
            })()"#
        ),
        "true|true|height: 5px;|object"
    );
}

/// `window.find` selects the next match within a text node after the selection (wrapping on
/// request); WPT editing crashtests call it.
#[test]
fn window_find_selects_matches() {
    let env = Env::new("http://svg.test/");
    assert_eq!(
        env.eval(
            r#"(() => {
              document.body.innerHTML = "<p>one AAA two aaa</p>";
              const out = [find("aaa"), getSelection().toString(), find("aaa"), getSelection().anchorOffset];
              out.push(find("AAA", true), find("zzz"), find("aaa", false, false, true), getSelection().anchorOffset);
              return out.join("|");
            })()"#
        ),
        "true|AAA|true|12|false|false|true|4"
    );
}

/// `document.designMode`: the document's tree is editable (not shadow trees), the body is the
/// editing host of its content (`delete` after `selectAll` empties it, it is not removed), and a
/// line break in a `<script>` of the head is a linefeed (WPT editing designMode crashtests,
/// insertparagraph-in-child-of-head).
#[test]
fn design_mode_edits_the_document() {
    let env = Env::new("http://svg.test/");
    assert_eq!(
        env.eval(
            r#"(() => {
              document.body.innerHTML = "<p>one</p><div id=h></div>";
              const p = document.querySelector("p");
              const shadow = document.getElementById("h").attachShadow({ mode: "open" });
              shadow.innerHTML = "<span>s</span>";
              const out = [document.designMode, p.isContentEditable];
              document.designMode = "ON";
              out.push(document.designMode, p.isContentEditable, shadow.firstChild.isContentEditable);
              document.execCommand("selectAll");
              document.execCommand("delete");
              out.push(document.body !== null, document.body.textContent);
              const s = document.createElement("script");
              document.head.appendChild(s);
              getSelection().collapse(s, 0);
              document.execCommand("insertParagraph");
              out.push(JSON.stringify(s.textContent), s.children.length);
              document.designMode = "off";
              out.push(document.designMode, document.body.isContentEditable);
              return out.join("|");
            })()"#
        ),
        "off|false|on|true|false|true||\"\\n\"|0|off|false"
    );
}

/// Trusted Types, slice 1: `trustedTypes.createPolicy`, the Trusted* values (no constructor,
/// stringify to their data), `isHTML`/`isScript`/`isScriptURL`, `emptyHTML`/`emptyScript`,
/// `defaultPolicy`, `getAttributeType`/`getPropertyType`.
#[test]
fn trusted_types_policies_and_values() {
    let env = Env::new("http://svg.test/");
    assert_eq!(
        env.eval(
            r#"(() => {
              const p = trustedTypes.createPolicy("p", { createHTML: (s, extra) => s + "|" + extra, createScriptURL: () => null });
              const h = p.createHTML("a", "b");
              const out = [String(h), h instanceof TrustedHTML, trustedTypes.isHTML(h), trustedTypes.isScript(h), JSON.stringify(h)];
              out.push(JSON.stringify(String(p.createScriptURL("x"))), p.name, trustedTypes.defaultPolicy);
              try { p.createScript("x"); out.push("no throw"); } catch (e) { out.push(e.name); }
              try { new TrustedScript(); out.push("no throw"); } catch (e) { out.push(e.name); }
              const d = trustedTypes.createPolicy("default", {});
              out.push(trustedTypes.defaultPolicy === d, trustedTypes.emptyHTML === trustedTypes.emptyHTML, trustedTypes.isScript(trustedTypes.emptyScript));
              out.push(trustedTypes.getAttributeType("script", "src"), trustedTypes.getAttributeType("div", "onclick"), trustedTypes.getPropertyType("iframe", "srcdoc"), trustedTypes.getPropertyType("div", "title"));
              return out.join("|");
            })()"#
        ),
        "a|b|true|true|false|\"a|b\"|\"\"|p||TypeError|TypeError|true|true|true|TrustedScriptURL|TrustedScript|TrustedHTML|"
    );
}

/// Trusted Types, slice 2: a `<meta http-equiv="Content-Security-Policy">` that joins the head
/// with `require-trusted-types-for 'script'` makes the HTML sinks take TrustedHTML or what the
/// default policy makes of a string (a TypeError without one); `trusted-types` names policies.
#[test]
fn trusted_types_enforced_by_meta_csp() {
    let env = Env::new("http://svg.test/");
    // As in a browsing context: insertions process `<meta>` elements.
    env.page.dom.borrow_mut().set_resource_tracking(true);
    assert_eq!(
        env.eval(
            r#"(() => {
              const out = [];
              const div = document.createElement("div");
              div.innerHTML = "<b>free</b>";
              out.push(div.innerHTML);
              const meta = document.createElement("meta");
              meta.setAttribute("http-equiv", "Content-Security-Policy");
              meta.setAttribute("content", "require-trusted-types-for 'script'; trusted-types p default");
              document.head.appendChild(meta);
              try { div.innerHTML = "<i>x</i>"; out.push("no throw"); } catch (e) { out.push(e.name); }
              const p = trustedTypes.createPolicy("p", { createHTML: s => s });
              div.innerHTML = p.createHTML("<u>ok</u>");
              out.push(div.innerHTML);
              try { trustedTypes.createPolicy("q", {}); out.push("no throw"); } catch (e) { out.push(e.name); }
              trustedTypes.createPolicy("default", { createHTML: s => s.toUpperCase() });
              div.insertAdjacentHTML("beforeend", "<s>a</s>");
              out.push(div.innerHTML);
              return out.join("|");
            })()"#
        ),
        "<b>free</b>|TypeError|<u>ok</u>|TypeError|<u>ok</u><s>A</s>"
    );
}

/// `document.domain`: the origin's host; setting a registrable domain suffix passes the checks
/// and changes nothing (origin-keyed agent clusters, Chromium's default), anything else is a
/// SecurityError; a document without a browsing context cannot set it.
#[test]
fn document_domain_is_origin_keyed() {
    let env = Env::new("http://a.example.com/");
    assert_eq!(
        env.eval(
            r#"(() => {
              const out = [document.domain];
              const set = v => { try { document.domain = v; return "ok"; } catch (e) { return e.name; } };
              out.push(set("example.com"), document.domain, set("a.example.com"), set("com"), set("other.com"), set("b.a.example.com"));
              const doc = document.implementation.createHTMLDocument("");
              try { doc.domain = "example.com"; out.push("no throw"); } catch (e) { out.push(e.name); }
              return out.join("|");
            })()"#
        ),
        "a.example.com|ok|a.example.com|ok|SecurityError|SecurityError|SecurityError|SecurityError"
    );
}

/// Trusted Types, script sinks: with `require-trusted-types-for 'script'`, a `<script>`'s text
/// (`text`, `textContent`, `innerText`) takes TrustedScript, its `src` TrustedScriptURL, and
/// `setAttribute` of an event handler content attribute, `iframe srcdoc` or `script src` the
/// matching type, else what the default policy makes of the string (a TypeError without one).
#[test]
fn trusted_types_script_and_attribute_sinks() {
    let env = Env::new("http://svg.test/");
    env.page.dom.borrow_mut().set_resource_tracking(true);
    assert_eq!(
        env.eval(
            r#"(() => {
              const meta = document.createElement("meta");
              meta.setAttribute("http-equiv", "Content-Security-Policy");
              meta.setAttribute("content", "require-trusted-types-for 'script'");
              document.head.appendChild(meta);
              const out = [];
              const t = f => { try { f(); return "ok"; } catch (e) { return e.name; } };
              const s = document.createElement("script");
              const d = document.createElement("div");
              out.push(t(() => s.text = "1"), t(() => s.textContent = "1"), t(() => s.innerText = "1"), t(() => s.src = "/x.js"));
              out.push(t(() => d.setAttribute("onclick", "x()")), t(() => d.setAttribute("title", "free")), t(() => document.createElement("iframe").setAttribute("srcdoc", "<p>")));
              const p = trustedTypes.createPolicy("p", { createScript: v => v, createScriptURL: v => v });
              s.text = p.createScript("a = 1");
              d.setAttribute("onclick", p.createScript("b()"));
              s.src = p.createScriptURL("/y.js");
              out.push(s.text, d.getAttribute("onclick"), s.getAttribute("src"));
              trustedTypes.createPolicy("default", { createScript: (v, type, sink) => v + "//" + type + "|" + sink });
              s.textContent = "c";
              out.push(s.textContent);
              return out.join("|");
            })()"#
        ),
        "TypeError|TypeError|TypeError|TypeError|TypeError|ok|TypeError|a = 1|b()|/y.js|c//TrustedScript|HTMLScriptElement textContent"
    );
}

/// Filter primitives have their Filter Effects interfaces: `in`/`result` as `SVGAnimatedString`s,
/// numbers (pairs such as `stdDeviation` and `order` split into X and Y) as `SVGAnimatedNumber`s
/// and `SVGAnimatedInteger`s with their initial values, booleans and enumerations, and
/// `setStdDeviation` (WPT css/filter-effects idlharness, svgfeblendelement-mode-001).
#[test]
fn filter_primitives_reflect_animated_attributes() {
    let env = Env::new("http://svg.test/");
    assert_eq!(
        env.eval(
            r##"(() => {
              const ns = "http://www.w3.org/2000/svg";
              const blend = document.createElementNS(ns, "feBlend");
              const out = [blend instanceof SVGFEBlendElement, blend.in1 === blend.in1, blend.mode.baseVal];
              blend.setAttribute("in", "SourceAlpha");
              blend.setAttribute("mode", "luminosity");
              out.push(blend.in1.baseVal, blend.in1.animVal, blend.mode.baseVal === SVGFEBlendElement.SVG_FEBLEND_MODE_LUMINOSITY);
              blend.result.baseVal = "r1";
              out.push(blend.getAttribute("result"));
              const blur = document.createElementNS(ns, "feGaussianBlur");
              out.push(blur.stdDeviationX.baseVal, blur.edgeMode.baseVal);
              blur.setAttribute("stdDeviation", "3");
              out.push(blur.stdDeviationX.baseVal, blur.stdDeviationY.baseVal);
              blur.stdDeviationY.baseVal = 1.5;
              out.push(blur.getAttribute("stdDeviation"));
              blur.setStdDeviation(4, 5);
              out.push(blur.stdDeviationX.baseVal, blur.stdDeviationY.baseVal);
              try { blur.stdDeviationX.baseVal = Infinity; out.push("no throw"); } catch (e) { out.push(e.constructor.name); }
              const conv = document.createElementNS(ns, "feConvolveMatrix");
              out.push(conv.orderX.baseVal, conv.orderY.baseVal, conv.preserveAlpha.baseVal);
              conv.setAttribute("order", "5 2");
              conv.setAttribute("preserveAlpha", "true");
              out.push(conv.orderX.baseVal, conv.orderY.baseVal, conv.preserveAlpha.baseVal);
              const func = document.createElementNS(ns, "feFuncA");
              out.push(func instanceof SVGComponentTransferFunctionElement, func.slope.baseVal, func.type.baseVal);
              const img = document.createElementNS(ns, "feImage");
              img.setAttributeNS("http://www.w3.org/1999/xlink", "xlink:href", "#a");
              out.push(img.href.baseVal, document.createElementNS(ns, "feTurbulence").numOctaves.baseVal);
              return out.join("|");
            })()"##
        ),
        "true|true|1|SourceAlpha|SourceAlpha|true|r1|0|3|3|3|3 1.5|4|5|TypeError|3|3|false|5|2|true|true|1|1|#a|1"
    );
}

/// Trusted Types violations fire `securitypolicyviolation` (a SecurityPolicyViolationEvent) at the
/// document in a later task: sink violations with blockedURI "trusted-types-sink" and the sink
/// and a sample, policy creation violations with "trusted-types-policy" and the name.
#[test]
fn trusted_types_violations_are_reported() {
    let env = Env::new("http://svg.test/");
    env.page.dom.borrow_mut().set_resource_tracking(true);
    env.eval(
        r#"globalThis.reports = [];
           document.addEventListener("securitypolicyviolation", e => reports.push([e instanceof SecurityPolicyViolationEvent, e.blockedURI, e.effectiveDirective, e.sample, e.disposition].join(",")));
           const meta = document.createElement("meta");
           meta.setAttribute("http-equiv", "Content-Security-Policy");
           meta.setAttribute("content", "require-trusted-types-for 'script'; trusted-types a");
           document.head.appendChild(meta);
           try { document.createElement("div").innerHTML = "<b>x</b>"; } catch (e) {}
           try { trustedTypes.createPolicy("b", {}); } catch (e) {}
           String(reports.length)"#,
    );
    env.until("reports.length === 2");
    assert_eq!(
        env.eval("reports.join('|')"),
        "true,trusted-types-sink,require-trusted-types-for,Element innerHTML|<b>x</b>,enforce|true,trusted-types-policy,trusted-types,b,enforce"
    );
}

/// Trusted Types for eval and Function (JSC patch 0035): under `require-trusted-types-for
/// 'script'` a string is refused with an EvalError unless the default policy gives it back
/// unchanged; a TrustedScript is evaluated as its text; a Function of TrustedScripts compiles.
#[test]
fn trusted_types_guard_eval_and_function() {
    if !pyro_js::compile_strings_hooks_available() {
        eprintln!("skipped: JavaScriptCore without patch 0035");
        return;
    }
    let env = Env::new("http://svg.test/");
    env.page.dom.borrow_mut().set_resource_tracking(true);
    assert_eq!(env.eval("eval(trustedTypes.createPolicy('p0', { createScript: s => s }).createScript('6 * 7'))"), "42");
    env.eval(
        r#"const meta = document.createElement("meta");
           meta.setAttribute("http-equiv", "Content-Security-Policy");
           meta.setAttribute("content", "require-trusted-types-for 'script'");
           document.head.appendChild(meta);
           globalThis.p = trustedTypes.createPolicy("p", { createScript: s => s });
           globalThis.outcome = (f) => { try { return String(f()); } catch (e) { return e.name; } };"#,
    );
    // The level follows the document before the next classic script; set it as one would.
    {
        let scope = env.realm.scope().unwrap();
        pyro_webapi::trusted_types::sync_compile_strings(&scope, &env.page);
    }
    assert_eq!(
        env.eval(
            r#"[outcome(() => eval("1 + 1")), outcome(() => (0, eval)("1 + 2")), outcome(() => new Function("return 3")()),
                outcome(() => eval(p.createScript("1 + 3"))), outcome(() => new Function(p.createScript("return 5"))())].join()"#
        ),
        "EvalError,EvalError,EvalError,4,5"
    );
    env.eval(r#"trustedTypes.createPolicy("default", { createScript: s => s.includes("ok") ? s : s + ";bad" })"#);
    assert_eq!(
        env.eval(r#"[outcome(() => eval("'ok'")), outcome(() => eval("'no'"))].join()"#),
        "ok,EvalError"
    );
}

/// Length attributes are `SVGAnimatedLength`s whose `SVGLength`s reflect the attribute: units,
/// `value` in user units (percentages of the nearest `svg` viewBox, `em` of the font size),
/// `valueAsString`, `newValueSpecifiedUnits`, `convertToSpecifiedUnits`, initial values, and a
/// read-only `animVal`. rect/circle/ellipse/line are SVGGeometryElements.
#[test]
fn length_attributes_reflect_as_svg_lengths() {
    let env = Env::new("http://svg.test/");
    assert_eq!(
        env.eval(
            r##"(() => {
              const ns = "http://www.w3.org/2000/svg";
              const svg = document.createElementNS(ns, "svg");
              svg.setAttribute("viewBox", "0 0 200 100");
              const rect = document.createElementNS(ns, "rect");
              svg.appendChild(rect);
              document.body.appendChild(svg);
              const out = [rect instanceof SVGGeometryElement, rect instanceof SVGGraphicsElement, rect.width === rect.width, rect.width.baseVal === rect.width.baseVal];
              rect.setAttribute("width", "50%");
              const w = rect.width.baseVal;
              out.push(w.unitType === SVGLength.SVG_LENGTHTYPE_PERCENTAGE, w.valueInSpecifiedUnits, w.value, w.valueAsString);
              w.newValueSpecifiedUnits(SVGLength.SVG_LENGTHTYPE_IN, 1);
              out.push(rect.getAttribute("width"), w.value);
              w.convertToSpecifiedUnits(SVGLength.SVG_LENGTHTYPE_PX);
              out.push(rect.getAttribute("width"));
              w.valueAsString = "2cm";
              out.push(w.unitType, rect.width.animVal.valueAsString);
              try { w.valueAsString = "nope"; out.push("no throw"); } catch (e) { out.push(e.name); }
              try { rect.width.animVal.value = 3; out.push("no throw"); } catch (e) { out.push(e.name); }
              rect.setAttribute("rx", "2em");
              out.push(rect.rx.baseVal.value);
              w.value = 10;
              out.push(rect.getAttribute("width"));
              const filter = document.createElementNS(ns, "filter");
              out.push(filter.x.baseVal.valueAsString, filter.width.baseVal.valueAsString);
              out.push(document.createElementNS(ns, "feFlood").height.baseVal.valueAsString);
              out.push(document.createElementNS(ns, "circle").r.baseVal.unitType);
              return out.join("|");
            })()"##
        ),
        "true|true|true|true|true|50|100|50%|1in|96|96px|6|2cm|SyntaxError|NoModificationAllowedError|32|0.26458332cm|-10%|120%|100%|1"
    );
}

/// `color-mix()` percentages that sum to zero mix the colors equally into a fully transparent color
/// (css-color-5), rather than transparent black (WPT css-color color-computed-color-mix-function).
#[test]
fn color_mix_with_zero_percentages_is_an_equal_transparent_mix() {
    let env = Env::new("http://color.test/");
    assert_eq!(
        env.eval(
            r#"(() => {
              const el = document.createElement("div");
              document.body.appendChild(el);
              const out = [];
              for (const v of ["color-mix(in srgb, red 0%, blue 0%)", "color-mix(in srgb, red calc(0%), blue calc(0%))", "color-mix(in srgb, red 50%, blue 50%)"]) {
                el.style.color = v;
                out.push(getComputedStyle(el).color);
              }
              return out.join("|");
            })()"#
        ),
        "color(srgb 0.5 0 0.5 / 0)|color(srgb 0.5 0 0.5 / 0)|color(srgb 0.5 0 0.5)"
    );
}

/// Relative colors keep the origin's missing components (`none`, and hues that become powerless in
/// the conversion), and legacy `rgb()` with `none` serializes as `color(srgb ...)` (WPT css-color
/// color-computed-none, color-computed-relative-color, color-computed-rgb).
#[test]
fn relative_colors_keep_missing_components() {
    let env = Env::new("http://color.test/");
    assert_eq!(
        env.eval(
            r#"(() => {
              const el = document.createElement("div");
              document.body.appendChild(el);
              const out = [];
              for (const v of ["lab(from lab(none none none) l a b)", "rgb(128 none none)", "lch(from hsl(180 50% 0) l c h)", "hsl(from hsl(180 0 50%) h s l)"]) {
                el.style.color = v;
                out.push(getComputedStyle(el).color);
              }
              return out.join("|");
            })()"#
        ),
        "lab(none none none)|color(srgb 0.501961 none none)|lch(0 0 none)|color(srgb 0.501961 0.501961 0.501961)"
    );
}

/// A color function with a `calc()` component keeps its function form as its specified value, the
/// `calc()`s simplified and the other components as the numbers the function resolves them to
/// (css-color-4 § 15, as Chromium); legacy `rgb()`, `hsl()` and `hwb()` with numeric `calc()`s
/// still resolve; computed values resolve, with a NaN lightness, component or hue as 0.
/// WPT css-color `parsing/color-valid-lab`, `-color-function`, `-rgb`, `-hsl`, `-hwb`, `color-computed-lab`.
#[test]
fn color_functions_keep_calc_in_their_specified_value() {
    let env = Env::new("http://color.test/");
    assert_eq!(
        env.eval(
            r#"(() => {
              const el = document.createElement("div");
              document.body.appendChild(el);
              const out = [];
              for (const v of [
                "lab(calc(50 * 3) calc(0.5 - 1) calc(1.5) / calc(-0.5 + 1))",
                "lab(200 calc(50%) 50%)",
                "lch(calc(50%) -100 50deg / 50%)",
                "color(srgb calc(50%) 50% 0.5)",
                "oklch(none 0.2 calc(0.5))",
                "rgb(calc(infinity), 0, 0)",
                "rgb(calc(50% + (sign(1em - 10px) * 10%)), 400%, 0%, 50%)",
                "hsl(calc(0 / 0) 100% 50%)",
                "lab(50 20 30)",
              ]) {
                el.style.color = "";
                el.style.color = v;
                out.push(el.style.color);
              }
              for (const v of ["lab(calc(NaN) 0 0)", "lch(50 10 calc(infinity))", "lab(calc(50 * 3) 0 0)"]) {
                el.style.color = v;
                out.push(getComputedStyle(el).color);
              }
              return out.join("|");
            })()"#
        ),
        "lab(calc(150) calc(-0.5) calc(1.5) / calc(0.5))|lab(100 calc(50%) 62.5)|lch(calc(50%) 0 50 / 0.5)|color(srgb calc(50%) 0.5 0.5)|oklch(none 0.2 calc(0.5))|rgb(255, 0, 0)|rgb(calc(50% + (10% * sign(1em - 10px))) 255 0 / 0.5)|rgb(255, 0, 0)|lab(50 20 30)|lab(0 0 0)|lch(50 10 0)|lab(100 0 0)"
    );
}

/// DOM IDL details (WPT dom/idlharness): operations throw a TypeError when called with fewer
/// arguments than they require; Range and StaticRange inherit their boundary points from
/// AbstractRange; Element's `slot` is unscopable; the Fullscreen API exists, with nothing ever
/// fullscreen (requests and exits reject).
#[test]
fn dom_idl_arguments_abstract_range_and_fullscreen() {
    let env = Env::new("http://idl.test/");
    assert_eq!(
        env.eval(
            r#"(() => {
              const out = [];
              const name = (f) => { try { f(); return "ok"; } catch (e) { return e.name; } };
              out.push(name(() => document.body.contains()), name(() => document.body.isEqualNode()),
                       name(() => document.lookupPrefix()), name(() => document.body.classList.contains()),
                       name(() => document.body.children.item()), name(() => document.createTextNode("ab").splitText()),
                       name(() => document.evaluate("/")), name(() => AbortSignal.timeout()));
              out.push(name(() => document.body.contains(null)));
              const r = document.createRange();
              out.push(Object.getPrototypeOf(Range.prototype) === AbstractRange.prototype,
                       Object.getPrototypeOf(StaticRange.prototype) === AbstractRange.prototype,
                       !Object.hasOwn(Range.prototype, "startOffset"), r.collapsed, r.startContainer === document);
              const sr = new StaticRange({ startContainer: document.body, startOffset: 0, endContainer: document.body, endOffset: 0 });
              out.push(sr.collapsed, sr.endContainer === document.body);
              out.push(name(() => Object.getOwnPropertyDescriptor(AbstractRange.prototype, "startOffset").get.call({})));
              out.push(Element.prototype[Symbol.unscopables].slot, Document.prototype[Symbol.unscopables].fullscreen);
              out.push(document.fullscreenEnabled, document.fullscreen, document.fullscreenElement, "onfullscreenchange" in document.body);
              return out.join("|");
            })()"#
        ),
        "TypeError|TypeError|TypeError|TypeError|TypeError|TypeError|TypeError|TypeError|ok|true|true|true|true|true|true|true|TypeError|true|true|false|false||true"
    );
    assert_eq!(
        env.eval(
            r#"(async () => {
              const out = [];
              try { await document.body.requestFullscreen(); out.push("resolved"); } catch (e) { out.push(e.name); }
              try { await document.exitFullscreen(); out.push("resolved"); } catch (e) { out.push(e.name); }
              globalThis.fsOut = out.join("|");
            })(), "started""#
        ),
        "started"
    );
    env.until("globalThis.fsOut !== undefined");
    assert_eq!(env.eval("fsOut"), "TypeError|TypeError");
}

/// A class change on an element above a `:has()` anchor restyles the anchor when a complex selector
/// inside the argument reaches it (WPT css/selectors is-pseudo-containing-complex-in-has).
#[test]
fn has_invalidation_reaches_ancestors_of_the_anchor() {
    let env = Env::new("http://has.test/");
    assert_eq!(
        env.eval(
            r##"(() => {
              document.head.insertAdjacentHTML("beforeend", "<style>div { color: grey } .red:has(#descendant:is(.a .b)) { color: red }</style>");
              document.body.innerHTML = '<div id="parent"><div id="scope" class="red"><div><div id="descendant" class="b"></div></div></div></div>';
              const scope = document.getElementById("scope");
              const out = [getComputedStyle(scope).color];
              document.getElementById("parent").classList.add("a");
              out.push(getComputedStyle(scope).color);
              document.getElementById("parent").classList.remove("a");
              out.push(getComputedStyle(scope).color);
              return out.join("|");
            })()"##
        ),
        "rgb(128, 128, 128)|rgb(255, 0, 0)|rgb(128, 128, 128)"
    );
}

/// Processing instruction attributes (WPT dom/nodes/processing-instruction-attributes): the data
/// read as quoted pairs; writes rewrite it in place as characterData mutations; invalid names throw.
#[test]
fn processing_instruction_attributes() {
    let env = Env::new("http://pi.test/");
    assert_eq!(
        env.eval(
            r##"(() => {
              const pi = document.createProcessingInstruction("t", 'a="b" x="yy"');
              const mo = new MutationObserver(() => {});
              mo.observe(pi, { characterData: true, attributes: true });
              const out = [pi.hasAttributes(), pi.getAttributeNames().join(","), pi.getAttribute("x"), pi.getAttribute("X")];
              pi.setAttribute("x", 'q"<');
              out.push(pi.data, pi.getAttribute("x"), mo.takeRecords().map(r => r.type).join());
              pi.removeAttribute("x");
              pi.toggleAttribute("c");
              out.push(pi.data, document.createProcessingInstruction("t", "a=b").hasAttributes());
              try { pi.setAttribute("a=", "v"); out.push("no throw"); } catch (e) { out.push(e.name); }
              return out.join("|");
            })()"##
        ),
        r#"true|a,x|yy||a="b" x="q&quot;&lt;"|q"<|characterData|a="b" c=""|false|InvalidCharacterError"#
    );
}

/// `:enabled`/`:disabled` in style matching follow HTML: options and optgroups, and the controls a
/// disabled fieldset or optgroup disables; `:default` takes `checked` only on checkboxes and radios
/// (WPT css/selectors invalidation has-with-pseudo-class, input-pseudo-classes-in-has).
#[test]
fn enabled_disabled_and_default_in_style_matching() {
    let env = Env::new("http://states.test/");
    assert_eq!(
        env.eval(
            r##"(() => {
              document.head.insertAdjacentHTML("beforeend", "<style>* { color: black } :disabled { color: red } :enabled { color: green } :default { outline-color: blue }</style>");
              document.body.innerHTML = '<fieldset id=fs><input id=inner></fieldset><select><optgroup id=og><option id=op>x</option></optgroup></select><input id=text checked><input id=box type=checkbox checked>';
              const c = (id) => getComputedStyle(document.getElementById(id)).color;
              const out = [c("inner"), c("og"), c("op"), c("fs")];
              document.getElementById("fs").disabled = true;
              document.getElementById("og").disabled = true;
              out.push(c("inner"), c("og"), c("op"), c("fs"));
              document.getElementById("fs").disabled = false;
              out.push(c("inner"));
              const o = (id) => getComputedStyle(document.getElementById(id)).outlineColor;
              out.push(o("text") === o("inner"), o("box"));
              return out.join("|");
            })()"##
        ),
        "rgb(0, 128, 0)|rgb(0, 128, 0)|rgb(0, 128, 0)|rgb(0, 128, 0)|rgb(255, 0, 0)|rgb(255, 0, 0)|rgb(255, 0, 0)|rgb(255, 0, 0)|rgb(0, 128, 0)|true|rgb(0, 0, 255)"
    );
}

/// `:focus` matches the shadow hosts whose shadow trees hold the focused element, at any depth
/// (WPT shadow-dom/focus/focus-pseudo-matches-on-shadow-host).
#[test]
fn focus_matches_shadow_hosts_of_the_focused_element() {
    let env = Env::new("http://focus.test/");
    assert_eq!(
        env.eval(
            r##"(() => {
              const outer = document.createElement("div");
              document.body.appendChild(outer);
              const inner = document.createElement("div");
              outer.attachShadow({ mode: "open" }).appendChild(inner);
              const input = document.createElement("input");
              inner.attachShadow({ mode: "closed" }).appendChild(input);
              const other = document.createElement("input");
              document.body.appendChild(other);
              input.focus();
              const out = [outer.matches(":focus"), inner.matches(":focus"), document.body.matches(":focus"), document.querySelector(":focus") === outer];
              other.focus();
              out.push(outer.matches(":focus"), other.matches(":focus"));
              return out.join("|");
            })()"##
        ),
        "true|true|false|true|false|true"
    );
}

/// ARIA element reflection (WPT html/dom/aria-element-reflection): ids resolve in the element's
/// tree, explicitly set elements win while the attribute reads "", arrays are frozen and the same
/// object while their elements stay the same, and references out of scope read as null.
#[test]
fn aria_element_reflection() {
    let env = Env::new("http://aria.test/");
    assert_eq!(
        env.eval(
            r##"(() => {
              document.body.innerHTML = '<div id=l1>a</div><div id=l2>b</div><input id=in aria-labelledby="l1 missing l2" aria-activedescendant="l2">';
              const input = document.getElementById("in"), l1 = document.getElementById("l1"), l2 = document.getElementById("l2");
              const out = [input.ariaLabelledByElements.map(e => e.id).join(","), input.ariaActiveDescendantElement === l2,
                           input.ariaLabelledByElements === input.ariaLabelledByElements, Object.isFrozen(input.ariaLabelledByElements),
                           input.ariaOwnsElements, input.ariaControlsElements];
              input.ariaActiveDescendantElement = l1;
              l1.id = "renamed";
              out.push(input.getAttribute("aria-activedescendant"), input.ariaActiveDescendantElement === l1);
              l1.remove();
              out.push(input.ariaActiveDescendantElement);
              input.ariaDescribedByElements = [l2];
              out.push(input.ariaDescribedByElements[0] === l2);
              input.setAttribute("aria-describedby", "l2 l2");
              out.push(input.ariaDescribedByElements.length);
              input.ariaDescribedByElements = null;
              out.push(input.hasAttribute("aria-describedby"));
              return out.join("|");
            })()"##
        ),
        "l1,l2|true|true|true||||true||true|2|false"
    );
}

/// SVG lengths (round 5): a value shorter than a unit name is a number (`x="5"` read as invalid and
/// gave the initial value), `image`, `use`, `foreignObject` and the text content elements have
/// their interfaces and SVGAnimatedLengths, `createSVGLength()` gives a length that reflects no
/// attribute, CSS units without a constant (`rem`, `vw`, `q`) convert with `unitType` 0, and
/// `newValueSpecifiedUnits` converts both arguments before it checks the unit.
/// WPT svg/types/scripted `SVGAnimatedLength-initial-values`, `SVGLength`, `SVGLength-px`,
/// `SVGLength-rem`, `SVGLength-viewport`.
#[test]
fn svg_lengths_of_one_character_new_interfaces_detached_lengths_and_css_units() {
    let env = Env::new("http://svg.test/");
    assert_eq!(
        env.eval(
            r#"(() => {
              const ns = "http://www.w3.org/2000/svg";
              const make = (name) => document.createElementNS(ns, name);
              const rect = make("rect");
              rect.setAttribute("x", "5");
              const out = [rect.x.baseVal.value, make("marker").markerWidth.baseVal.valueAsString];
              const image = make("image"), use = make("use"), foreign = make("foreignObject");
              const text = make("text"), tspan = make("tspan"), path = make("textPath");
              out.push(image instanceof SVGImageElement, use instanceof SVGUseElement, foreign instanceof SVGForeignObjectElement);
              out.push(text instanceof SVGTextElement && text instanceof SVGTextPositioningElement && text instanceof SVGTextContentElement);
              out.push(tspan instanceof SVGTSpanElement, path instanceof SVGTextPathElement && !(path instanceof SVGTextPositioningElement));
              image.setAttribute("width", "42");
              out.push(image.width.baseVal.value, use.x.baseVal.valueAsString, text.textLength.baseVal.value, path.startOffset.baseVal.value);
              const length = make("svg").createSVGLength();
              out.push(length.unitType, length.valueAsString);
              length.valueAsString = "2px";
              length.convertToSpecifiedUnits(SVGLength.SVG_LENGTHTYPE_PERCENTAGE);
              out.push(length.valueAsString);
              length.valueAsString = "2em";
              out.push(length.value);
              rect.setAttribute("x", "2rem");
              out.push(rect.x.baseVal.unitType, rect.x.baseVal.value);
              rect.setAttribute("x", "4Q");
              out.push(rect.x.baseVal.value.toFixed(2));
              try { length.newValueSpecifiedUnits("aString", "aString"); out.push("no throw"); } catch (e) { out.push(e.name); }
              try { length.newValueSpecifiedUnits("aString", 4); out.push("no throw"); } catch (e) { out.push(e.name); }
              return out.join("|");
            })()"#
        ),
        "5|3|true|true|true|true|true|true|42|0|0|0|1|0|2%|32|0|32|3.78|TypeError|NotSupportedError"
    );
}
