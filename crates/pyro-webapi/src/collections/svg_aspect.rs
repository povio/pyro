use super::*;
use pyro_dom::svg_aspect_ratio::PreserveAspectRatio;

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
        def.getter("baseVal", |s, el| reflected(s, el, false));
        def.getter("animVal", |s, el| reflected(s, el, true));
    }
}

fn reflected<'s>(s: &Scope<'s>, el: &NodeHold, readonly: bool) -> JsResult<Value<'s>> {
    let key = coll_identity(&el.page, 445 + u64::from(readonly), el.id);
    if let Some(value) = s.lookup_identity(key) {
        return Ok(value.value());
    }
    let data = AspectData { el: NodeHold::new(&el.page, el.id)?, readonly };
    Ok(s.wrap::<Aspect>(data, Some(key))?.value())
}

impl AspectData {
    fn aspect(&self) -> JsResult<PreserveAspectRatio> {
        self.el.page.dom.borrow().svg_aspect_ratio(self.el.id).js()
    }

    fn set(
        &self,
        value: Value<'_>,
        update: fn(PreserveAspectRatio, u16) -> Option<PreserveAspectRatio>,
    ) -> JsResult<()> {
        let value = to_u32(value)? as u16;
        if self.readonly {
            return Err(JsError::dom("NoModificationAllowedError", "The animated value is read-only."));
        }
        let next = update(self.aspect()?, value)
            .ok_or_else(|| JsError::Type("Unsupported preserveAspectRatio value.".into()))?;
        self.el.page.dom.borrow_mut().set_svg_aspect_ratio(self.el.id, next).js()
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
        def.accessor(
            "align",
            |s, d| Ok(s.number(f64::from(d.aspect()?.align()))),
            |_, d, value| d.set(value, PreserveAspectRatio::with_align),
        );
        def.accessor(
            "meetOrSlice",
            |s, d| Ok(s.number(f64::from(d.aspect()?.meet_or_slice()))),
            |_, d, value| d.set(value, PreserveAspectRatio::with_meet_or_slice),
        );
    }
}

pub(crate) fn animated_aspect<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let key = coll_identity(n.page(), 444, n.id);
    if let Some(value) = s.lookup_identity(key) {
        return Ok(value.value());
    }
    Ok(s.wrap::<AnimatedAspect>(NodeHold::new(n.page(), n.id)?, Some(key))?.value())
}
