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
