//! Playwright's injected selector engine, adjusted for this browser.
//!
//! Playwright runs locators with its own selector engine, installed into the
//! page's utility world as a 300 KB script (`injectedScriptSource.js`) and
//! driven through `Runtime.callFunctionOn`. Two of its habits cost most of a
//! locator's time here, on pages that have no shadow DOM at all:
//!
//! - `query` splits `#id span` at every combinator, queries the last compound
//!   (`span`: every row) and walks up the parents of each candidate with
//!   `parentElement` and `matches` (18,000 and 2,000 calls on a 2,000-row
//!   page), because its custom pseudo-classes may appear anywhere in the
//!   chain. Without them, the selector is plain CSS, and `querySelectorAll`
//!   on the document gives the same elements in the same order.
//! - `_queryCSS` pierces shadow DOM by reading `.shadowRoot` of every element
//!   under the scope (6,000 more wrappers and calls per query).
//!
//! The source is rewritten as it arrives in `Runtime.evaluate` when, and only
//! when, it contains these two methods exactly as Playwright 1.59.1 ships
//! them (`bench/package.json` pins it); another version keeps its script. The
//! fast paths ask `__lightningpandaShadowRootCount()`, which every isolated
//! world defines (`lp_webapi::install_world_helpers`), and step aside as soon
//! as the page has a shadow root, when the query has a scope clause, or when
//! its root is not the document.

use std::borrow::Cow;

/// Playwright 1.59.1 `SelectorEvaluatorImpl.query`, as shipped.
const QUERY_1_59: &str = r#"        const previousScoreMap = this._scoreMap;
        this._scoreMap = /* @__PURE__ */ new Map();
        let elements = this._querySimple(context, selector.simples[selector.simples.length - 1].selector);
        elements = elements.filter((element) => this._matchesParents(element, selector, selector.simples.length - 2, context));
"#;

const QUERY_1_59_FAST: &str = r#"        const previousScoreMap = this._scoreMap;
        this._scoreMap = /* @__PURE__ */ new Map();
        const __lpCss = __lightningpandaPlainCss(selector, context);
        if (__lpCss !== void 0) {
          this._scoreMap = previousScoreMap;
          return [...context.scope.querySelectorAll(__lpCss)];
        }
        let elements = this._querySimple(context, selector.simples[selector.simples.length - 1].selector);
        elements = elements.filter((element) => this._matchesParents(element, selector, selector.simples.length - 2, context));
"#;

/// Playwright 1.59.1 `SelectorEvaluatorImpl._queryCSS`'s shadow walk.
const QUERY_CSS_1_59: &str = r#"        if (root.shadowRoot)
          query(root.shadowRoot);
        for (const element of root.querySelectorAll("*")) {
          if (element.shadowRoot)
            query(element.shadowRoot);
        }
"#;

const QUERY_CSS_1_59_FAST: &str = r#"        if (root.shadowRoot)
          query(root.shadowRoot);
        if (__lightningpandaNoShadowRoots())
          return;
        for (const element of root.querySelectorAll("*")) {
          if (element.shadowRoot)
            query(element.shadowRoot);
        }
"#;

const CLASS_1_59: &str = "var SelectorEvaluatorImpl = class {\n";

/// The helpers, defined right before the evaluator class. A selector is
/// plain CSS when no compound uses a custom pseudo-class and every combinator
/// is one CSS has (`>=` is Playwright's own); its text is the compounds joined
/// again, each pair by the combinator Playwright stored on the first of them.
const HELPERS_1_59: &str = r#"function __lightningpandaNoShadowRoots() {
  const count = globalThis.__lightningpandaShadowRootCount;
  return typeof count === "function" && count() === 0;
}
function __lightningpandaPlainCss(selector, context) {
  if (!__lightningpandaNoShadowRoots())
    return void 0;
  const scope = context.scope;
  if (!scope || scope.nodeType !== 9 || context.originalScope !== void 0 && context.originalScope !== scope)
    return void 0;
  const simples = selector.simples;
  let css = "";
  for (let i = 0; i < simples.length; i++) {
    const simple = simples[i].selector;
    if (!simple || simple.functions.length)
      return void 0;
    const part = simple.css || "*";
    if (i === 0) {
      css = part;
      continue;
    }
    const combinator = simples[i - 1].combinator;
    if (combinator === "")
      css += " " + part;
    else if (combinator === ">" || combinator === "+" || combinator === "~")
      css += " " + combinator + " " + part;
    else
      return void 0;
  }
  return css;
}
var SelectorEvaluatorImpl = class {
"#;

/// Whether `expression` may be Playwright's injected script (a cheap test
/// before the exact one).
fn looks_like_injected_script(expression: &str) -> bool {
    expression.len() > 100_000 && expression.contains("SelectorEvaluatorImpl")
}

/// The expression with the fast paths, when it is the known injected script.
pub(crate) fn rewrite_injected_script(expression: &str) -> Cow<'_, str> {
    if !looks_like_injected_script(expression) {
        return Cow::Borrowed(expression);
    }
    let has = |s: &str| expression.matches(s).count() == 1;
    if !(has(QUERY_1_59) && has(QUERY_CSS_1_59) && has(CLASS_1_59)) {
        return Cow::Borrowed(expression);
    }
    let out = expression
        .replacen(QUERY_1_59, QUERY_1_59_FAST, 1)
        .replacen(QUERY_CSS_1_59, QUERY_CSS_1_59_FAST, 1)
        .replacen(CLASS_1_59, HELPERS_1_59, 1);
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> String {
        let filler = "x".repeat(100_001);
        format!("{filler}\n{CLASS_1_59}  query(context, s) {{\n{QUERY_1_59}  }}\n  _queryCSS(context, css) {{\n{QUERY_CSS_1_59}  }}\n}};\n")
    }

    #[test]
    fn rewrites_the_known_script_once() {
        let src = sample();
        let out = rewrite_injected_script(&src);
        assert!(matches!(out, Cow::Owned(_)));
        assert!(out.contains("__lightningpandaPlainCss(selector, context)"));
        assert!(out.contains("if (__lightningpandaNoShadowRoots())\n          return;"));
        assert_eq!(out.matches("function __lightningpandaPlainCss").count(), 1);
        assert_eq!(out.matches(CLASS_1_59).count(), 1);
    }

    #[test]
    fn leaves_other_scripts_alone() {
        assert!(matches!(rewrite_injected_script("1 + 1"), Cow::Borrowed(_)));
        let changed = sample().replace("this._matchesParents(element, selector", "this._matchesParents(element, sel");
        assert!(matches!(rewrite_injected_script(&changed), Cow::Borrowed(_)));
        let twice = format!("{}{}", sample(), QUERY_1_59);
        assert!(matches!(rewrite_injected_script(&twice), Cow::Borrowed(_)));
    }
}
