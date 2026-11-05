//! `CanvasRenderingContext2D` without a rasterizer: text measurement and
//! state.
//!
//! - `font` parses and serializes the CSS shorthand, defaulting to `10px sans-serif`.
//! - `measureText` uses the same text measure as the flow layout
//!   (`lp_dom::text_metrics`).
//! - `save`/`restore` keep the font and `textAlign`.
//! - Drawing calls are accepted and draw nothing.
//! - Pixel readback (`getImageData`) throws `NotSupportedError`, so no page
//!   receives invented pixels.
//!
//! Glyph bounding boxes are the font's ascent/descent, not per-glyph ink.

use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::text_metrics::{self, FontSpec};
use lp_dom::NodeId;
use lp_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Scope, Value};

use crate::node::node_value;
use crate::page::PageDom;
use crate::util::*;

const CONTEXT_2D: &str = "__lpCanvas2d";

#[derive(Clone)]
struct State {
    font: FontSpec,
    text_align: String,
}

pub struct ContextData {
    page: Rc<PageDom>,
    canvas: NodeId,
    state: RefCell<State>,
    stack: RefCell<Vec<State>>,
}

fn default_font() -> FontSpec {
    text_metrics::parse_font("10px sans-serif", 10.0).expect("default canvas font")
}

/// `HTMLCanvasElement.getContext(type)`: one 2D context per canvas; other
/// context types are unsupported (null).
pub fn get_context<'s>(s: &Scope<'s>, page: &Rc<PageDom>, canvas: NodeId, kind: &str) -> JsResult<Value<'s>> {
    if kind != "2d" {
        return Ok(s.null());
    }
    let element = node_value(s, page, Some(canvas))?;
    let element = element.as_object().ok_or(JsError::ContextDestroyed)?;
    if let Some(existing) = s.get_hidden(element, CONTEXT_2D)? {
        return Ok(existing);
    }
    let data = ContextData {
        page: page.clone(),
        canvas,
        state: RefCell::new(State { font: default_font(), text_align: "start".into() }),
        stack: RefCell::new(Vec::new()),
    };
    let context = s.wrap::<CanvasRenderingContext2D>(data, None)?;
    s.set_hidden(element, CONTEXT_2D, context.value())?;
    Ok(context.value())
}

pub struct CanvasRenderingContext2D;
impl JsClass for CanvasRenderingContext2D {
    const NAME: &'static str = "CanvasRenderingContext2D";
    type Data = ContextData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("canvas", |s, c| node_value(s, &c.page, Some(c.canvas)));
        def.accessor(
            "font",
            |s, c| Ok(s.string(&c.state.borrow().font.serialize())),
            |_s, c, v| {
                // Invalid values are ignored; em/% resolve against 10px.
                if let Some(font) = text_metrics::parse_font(&dom_string(v)?, 10.0) {
                    c.state.borrow_mut().font = font;
                }
                Ok(())
            },
        );
        def.accessor(
            "textAlign",
            |s, c| Ok(s.string(&c.state.borrow().text_align)),
            |_s, c, v| {
                let v = dom_string(v)?;
                if matches!(v.as_str(), "start" | "end" | "left" | "right" | "center") {
                    c.state.borrow_mut().text_align = v;
                }
                Ok(())
            },
        );
        def.method("measureText", 1, |s, c, a| {
            let text = arg_string(&a, 0, "CanvasRenderingContext2D.measureText")?;
            let state = c.state.borrow().clone();
            let font = &state.font;
            let width = if font.is_monospace() {
                text_metrics::monospace_width(&text, font.size)
            } else {
                text_metrics::text_width(&text, font.size, font.weight)
            };
            let (ascent, descent) = text_metrics::vertical_metrics(font.size);
            // Left-to-right: start/left align at x, end/right end at x.
            let left = match state.text_align.as_str() {
                "center" => width / 2.0,
                "end" | "right" => width,
                _ => 0.0,
            };
            let m = s.object();
            m.set_atom("width", s.number(width))?;
            m.set_atom("actualBoundingBoxLeft", s.number(left))?;
            m.set_atom("actualBoundingBoxRight", s.number(width - left))?;
            m.set_atom("actualBoundingBoxAscent", s.number(ascent))?;
            m.set_atom("actualBoundingBoxDescent", s.number(descent))?;
            m.set_atom("fontBoundingBoxAscent", s.number(ascent))?;
            m.set_atom("fontBoundingBoxDescent", s.number(descent))?;
            m.set_atom("emHeightAscent", s.number(ascent))?;
            m.set_atom("emHeightDescent", s.number(descent))?;
            m.set_atom("alphabeticBaseline", s.number(0.0))?;
            Ok(m.value())
        });
        def.method("save", 0, |s, c, _a| {
            let state = c.state.borrow().clone();
            c.stack.borrow_mut().push(state);
            Ok(s.undefined())
        });
        def.method("restore", 0, |s, c, _a| {
            if let Some(state) = c.stack.borrow_mut().pop() {
                *c.state.borrow_mut() = state;
            }
            Ok(s.undefined())
        });
        for name in [
            "fillRect",
            "strokeRect",
            "clearRect",
            "fillText",
            "strokeText",
            "beginPath",
            "closePath",
            "moveTo",
            "lineTo",
            "bezierCurveTo",
            "quadraticCurveTo",
            "arc",
            "arcTo",
            "ellipse",
            "rect",
            "roundRect",
            "fill",
            "stroke",
            "clip",
            "drawImage",
            "putImageData",
            "setTransform",
            "resetTransform",
            "transform",
            "translate",
            "rotate",
            "scale",
            "setLineDash",
        ] {
            def.method(name, 0, |s, _c, _a| Ok(s.undefined()));
        }
        def.method("getLineDash", 0, |s, _c, _a| Ok(s.array(&[])?.value()));
        for name in ["getImageData", "createImageData", "isPointInPath", "isPointInStroke"] {
            def.method(name, 0, move |_s, _c, _a| {
                Err(JsError::dom("NotSupportedError", "Canvas pixels are not available (no rasterizer)"))
            });
        }
    }
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<CanvasRenderingContext2D>();
}
