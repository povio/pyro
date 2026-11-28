//! Web Neural Network API (`navigator.ml`, `ML`, `MLContext`, `MLTensor`, `MLGraphBuilder`, `MLOperand`,
//! `MLGraph`): made in JS (webnn/core.js plus one file per operator family) on a realm's first use of
//! `navigator.ml` or of any of the interface names.
//!
//! This is a real CPU implementation: graphs are validated at build time and executed on typed arrays when
//! `MLContext.dispatch()` runs, with kernels computing in double precision and rounding once to the output
//! data type. Every context is CPU-backed whatever its `deviceType` or `powerPreference` hint says, so
//! `accelerated` is false; interop with WebGPU does not exist (pyro has no WebGPU).
//!
//! The module's source is assembled here from the files below: `webnn/core.js` is the factory (it documents
//! the operator registry the others code against), and every family file is one expression evaluating to
//! `(api) => { api.defineOp(...) }`. Add a family by adding its file to the `families` list. Natives:
//! `active()` (whether the realm's document is fully active).
use std::rc::Rc;

use pyro_js::{ClassDef, JsClass, JsError, JsResult, Object, Scope, Value};

use crate::lazy_module::{lazy_globals, LazyModule};
use crate::page::PageDom;

static MODULE: LazyModule = LazyModule {
    key: "lpWebNN",
    source: concat!(
        "(function (native) {\n'use strict';\nconst families = [\n",
        include_str!("webnn/elementwise.js"),
        "\n,\n",
        include_str!("webnn/activations.js"),
        "\n,\n",
        include_str!("webnn/reduce.js"),
        "\n,\n",
        include_str!("webnn/conv.js"),
        "\n,\n",
        include_str!("webnn/norm.js"),
        "\n,\n",
        include_str!("webnn/recurrent.js"),
        "\n,\n",
        include_str!("webnn/tensor_ops.js"),
        "\n,\n",
        include_str!("webnn/linear.js"),
        "\n];\nconst core =\n",
        include_str!("webnn/core.js"),
        "\n;\nnative.families = families;\nreturn core(native);\n})"
    ),
    url: "pyro://internal/webnn.js",
    natives: Some(natives),
};

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let native = s.object();
    let active = s.function("active", |s, _, _| Ok(s.boolean(crate::target::page_of(s)?.is_fully_active())))?;
    native.set_atom("active", active.value())?;
    // policy(): whether the document's permissions policy allows `webnn` (a refusal is reported).
    let policy = s.function("policy", |s, _, _| {
        let page = crate::target::page_of(s)?;
        Ok(s.boolean(crate::permissions_policy::check_feature(s, &page, "webnn")))
    })?;
    native.set_atom("policy", policy.value())?;
    Ok(native)
}

/// Adds the lazy interface names to pyro-browser's `Window` class (hidden outside a secure context by
/// `crate::secure_context`).
pub(crate) fn define_globals<W>(def: &mut ClassDef<W>)
where
    W: JsClass,
    W::Data: crate::window::WindowHost,
{
    lazy_globals!(def, MODULE, "ML", "MLContext", "MLGraph", "MLGraphBuilder", "MLOperand", "MLTensor");
}

/// `navigator.ml`: the realm's `ML` object in a secure context ([SameObject]).
pub(crate) fn ml<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    if page.is_disposed() {
        return Err(JsError::ContextDestroyed);
    }
    if !crate::secure_context::secure(s, page) {
        return Ok(s.undefined());
    }
    MODULE.module(s, page)?.get("ml")
}
