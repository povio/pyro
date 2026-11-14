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
