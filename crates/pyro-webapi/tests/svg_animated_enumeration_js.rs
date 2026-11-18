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
