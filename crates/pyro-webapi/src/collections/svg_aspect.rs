use super::*;

pub(super) fn register(engine: &pyro_js::Engine) {
    engine.register::<AnimatedAspect>();
    engine.register::<Aspect>();
}

struct AnimatedAspect;
struct Aspect;
struct AspectData {
    el: NodeHold,
    readonly: bool,
}

impl JsClass for AnimatedAspect {
    const NAME: &'static str = "SVGAnimatedPreserveAspectRatio";
    type Data = NodeHold;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        for (name, readonly) in [("baseVal", false), ("animVal", true)] {
            def.getter(name, move |s, el| {
                let key = coll_identity(&el.page, 445 + u64::from(readonly), el.id);
                if let Some(value) = s.lookup_identity(key) {
                    return Ok(value.value());
                }
                let data = AspectData { el: NodeHold::new(&el.page, el.id)?, readonly };
                Ok(s.wrap::<Aspect>(data, Some(key))?.value())
            });
        }
    }
}

impl JsClass for Aspect {
    const NAME: &'static str = "SVGPreserveAspectRatio";
    type Data = AspectData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        for (value, name) in [
            "SVG_PRESERVEASPECTRATIO_UNKNOWN",
            "SVG_PRESERVEASPECTRATIO_NONE",
            "SVG_PRESERVEASPECTRATIO_XMINYMIN",
            "SVG_PRESERVEASPECTRATIO_XMIDYMIN",
            "SVG_PRESERVEASPECTRATIO_XMAXYMIN",
            "SVG_PRESERVEASPECTRATIO_XMINYMID",
            "SVG_PRESERVEASPECTRATIO_XMIDYMID",
            "SVG_PRESERVEASPECTRATIO_XMAXYMID",
            "SVG_PRESERVEASPECTRATIO_XMINYMAX",
            "SVG_PRESERVEASPECTRATIO_XMIDYMAX",
            "SVG_PRESERVEASPECTRATIO_XMAXYMAX",
        ]
        .into_iter()
        .enumerate()
        {
            def.constant(name, value as f64);
        }
        def.constant("SVG_MEETORSLICE_UNKNOWN", 0.);
        def.constant("SVG_MEETORSLICE_MEET", 1.);
        def.constant("SVG_MEETORSLICE_SLICE", 2.);
        for (name, align) in [("align", true), ("meetOrSlice", false)] {
            def.accessor(
                name,
                move |s, data| {
                    let aspect = data.el.page.dom.borrow().svg_aspect_ratio(data.el.id).js()?;
                    Ok(s.number(f64::from(if align { aspect.align() } else { aspect.meet_or_slice() })))
                },
                move |_s, data, value| {
                    let value = to_u32(value)? as u16;
                    if data.readonly {
                        return Err(JsError::dom("NoModificationAllowedError", "The animated value is read-only."));
                    }
                    let mut dom = data.el.page.dom.borrow_mut();
                    let current = dom.svg_aspect_ratio(data.el.id).js()?;
                    let next = if align { current.with_align(value) } else { current.with_meet_or_slice(value) }
                        .ok_or_else(|| JsError::Type("Unsupported preserveAspectRatio value.".into()))?;
                    dom.set_svg_aspect_ratio(data.el.id, next).js()
                },
            );
        }
    }
}

pub(crate) fn animated_aspect<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let key = coll_identity(n.page(), 444, n.id);
    if let Some(value) = s.lookup_identity(key) {
        return Ok(value.value());
    }
    Ok(s.wrap::<AnimatedAspect>(NodeHold::new(n.page(), n.id)?, Some(key))?.value())
}
