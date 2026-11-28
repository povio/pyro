//! `SVGAnimationElement` and the interfaces of the animation elements (SVG Animations
//! https://svgwg.org/specs/animations/#InterfaceSVGAnimationElement): `<animate>`
//! (`SVGAnimateElement`), `<set>` (`SVGSetElement`), `<animateMotion>`
//! (`SVGAnimateMotionElement`) and `<animateTransform>` (`SVGAnimateTransformElement`).
//!
//! The timing is [`crate::smil`]'s: `beginElement()` and its siblings add an instance time to the
//! element's `begin` or `end` list, `getStartTime()` and `getSimpleDuration()` read its intervals.

use pyro_js::{ClassDef, JsClass, JsError, JsResult};

use crate::collections;
use crate::html::SvgElement;
use crate::node::{node_value, NodeData};
use crate::target::{event_handler, set_event_handler};

/// `SVGAnimationElement`: `SVGElement` plus `SVGTests`, the time control and the timing events.
pub struct SvgAnimationElement;
impl JsClass for SvgAnimationElement {
    const NAME: &'static str = "SVGAnimationElement";
    type Data = NodeData;
    type Parent = SvgElement;
    fn define(def: &mut ClassDef<Self>) {
        use pyro_dom::svg_string_list::StringListAttribute;
        def.getter("requiredExtensions", |s, n| collections::svg_string_list(s, n, StringListAttribute::RequiredExtensions));
        def.getter("systemLanguage", |s, n| collections::svg_string_list(s, n, StringListAttribute::SystemLanguage));
        // `href`/`xlink:href`'s element, else the parent (null when it names none).
        def.getter("targetElement", |s, n| {
            let target = crate::smil::target_element(n.page(), n.id);
            node_value(s, n.page(), target)
        });
        def.method("getStartTime", 0, |s, n, _a| match crate::smil::start_time(n.page(), n.id) {
            Some(seconds) => Ok(s.number(seconds)),
            None => Err(JsError::dom("InvalidStateError", "Failed to execute 'getStartTime' on 'SVGAnimationElement': the element has no current interval.")),
        });
        def.method("getCurrentTime", 0, |s, n, _a| Ok(s.number(crate::smil::current_time(n.page()))));
        def.method("getSimpleDuration", 0, |s, n, _a| match crate::smil::simple_duration(n.page(), n.id) {
            Some(seconds) => Ok(s.number(seconds)),
            None => Err(JsError::dom("NotSupportedError", "Failed to execute 'getSimpleDuration' on 'SVGAnimationElement': the simple duration is indefinite.")),
        });
        def.method("beginElement", 0, |s, n, _a| {
            crate::smil::add_instance(n.page(), n.id, false, 0.0);
            Ok(s.undefined())
        });
        def.method("beginElementAt", 1, |s, n, a| {
            a.require(1, "SVGAnimationElement.beginElementAt")?;
            let offset = finite_offset(a.get(0).to_number()?, "beginElementAt")?;
            crate::smil::add_instance(n.page(), n.id, false, offset);
            Ok(s.undefined())
        });
        def.method("endElement", 0, |s, n, _a| {
            crate::smil::add_instance(n.page(), n.id, true, 0.0);
            Ok(s.undefined())
        });
        def.method("endElementAt", 1, |s, n, a| {
            a.require(1, "SVGAnimationElement.endElementAt")?;
            let offset = finite_offset(a.get(0).to_number()?, "endElementAt")?;
            crate::smil::add_instance(n.page(), n.id, true, offset);
            Ok(s.undefined())
        });
        // The timing events' handlers (`onbegin` listens for `beginEvent`, ...).
        def.accessor("onbegin", |s, n| event_handler(s, &n.target, "beginEvent"), |s, n, v| set_event_handler(s, &n.target, "beginEvent", v));
        def.accessor("onend", |s, n| event_handler(s, &n.target, "endEvent"), |s, n, v| set_event_handler(s, &n.target, "endEvent", v));
        def.accessor("onrepeat", |s, n| event_handler(s, &n.target, "repeatEvent"), |s, n, v| set_event_handler(s, &n.target, "repeatEvent", v));
    }
}

/// An offset argument (`float`): non-finite is a TypeError, as WebIDL converts it.
fn finite_offset(offset: f64, method: &str) -> JsResult<f64> {
    if offset.is_finite() {
        Ok(offset)
    } else {
        Err(JsError::Type(format!("Failed to execute '{method}' on 'SVGAnimationElement': The provided float value is non-finite.").into()))
    }
}

macro_rules! animation_class {
    ($name:ident, $js:literal) => {
        pub struct $name;
        impl JsClass for $name {
            const NAME: &'static str = $js;
            type Data = NodeData;
            type Parent = SvgAnimationElement;
            fn define(_def: &mut ClassDef<Self>) {}
        }
    };
}

animation_class!(SvgAnimateElement, "SVGAnimateElement");
animation_class!(SvgSetElement, "SVGSetElement");
animation_class!(SvgAnimateMotionElement, "SVGAnimateMotionElement");
animation_class!(SvgAnimateTransformElement, "SVGAnimateTransformElement");

/// Registers the animation element classes with the engine (after [`crate::html::register`]: their
/// parent `SVGElement` is registered there).
pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<SvgAnimationElement>();
    engine.register::<SvgAnimateElement>();
    engine.register::<SvgSetElement>();
    engine.register::<SvgAnimateMotionElement>();
    engine.register::<SvgAnimateTransformElement>();
}
