# XPath

XPath 1.0 evaluation for `document.evaluate`, `document.createExpression`,
`document.createNSResolver`, `XPathEvaluator`, `XPathExpression` and
`XPathResult`. The engine is `lp_dom::xpath` (pure Rust over the `Dom`
store); the bindings are in `crates/lp-webapi/src/xpath.rs`. Both are ported
from upstream `src/browser/xpath/*.zig` and
`src/browser/webapi/XPath{Result,Expression,Evaluator}.zig`.

- Grammar: the full XPath 1.0 expression language: all 13 axes, `*`,
  `prefix:*`, QName, `node()`, `text()`, `comment()`,
  `processing-instruction(literal?)`, abbreviations (`.`, `..`, `@`, `//`),
  positional and boolean predicates (reverse axes count by proximity),
  filter expressions, unions, and the `or`/`and`/comparison/arithmetic
  operators with XPath §3.4 node-set comparison rules.
- Functions: all 27 core functions (`last`, `position`, `count`, `id`,
  `local-name`, `namespace-uri`, `name`, `string`, `concat`, `starts-with`,
  `contains`, `substring-before`, `substring-after`, `substring`,
  `string-length`, `normalize-space`, `translate`, `boolean`, `not`, `true`,
  `false`, `lang`, `number`, `sum`, `floor`, `ceiling`, `round`). Unknown
  functions, unknown axes and wrong argument counts are syntax errors.
- Node-sets are in document order without duplicates. Attributes are
  ordered after their element and before its children.
- HTML documents: an unprefixed name test matches HTML elements ASCII
  case-insensitively and does not match non-HTML elements (`//svg` is empty;
  use `//*[local-name()='svg']` or a resolved prefix), as in Blink/WebKit.
  Attribute names on HTML elements match case-insensitively. In XML
  documents unprefixed names match null-namespace elements exactly.
- Namespace resolver: `null`, a function, or an object with
  `lookupNamespaceURI` (such as a Node, which is what `createNSResolver`
  returns). Each prefix is resolved once when the expression is created;
  an unresolvable prefix throws `NamespaceError`.
- `XPathResult`: the 10 type constants on the interface and prototype,
  `resultType`, `numberValue`, `stringValue`, `booleanValue`,
  `singleNodeValue`, `invalidIteratorState`, `snapshotLength`,
  `iterateNext()`, `snapshotItem(i)`. `ANY_TYPE` resolves to number, string,
  boolean or `UNORDERED_NODE_ITERATOR_TYPE`.
- Errors: invalid expressions throw `SyntaxError` DOMException; requesting a
  node-set type for a scalar result, reading the wrong accessor, calling
  `snapshotItem()` without an index, or using a non-node-set where one is
  required (`1 | //a`, `count('x')`) throws TypeError; an unknown result
  type throws `NotSupportedError`.
- Iterator results are invalidated by a later tree mutation (node insertion
  or removal, attribute change, character-data change, tracked by
  `Dom::tree_version`): `invalidIteratorState` becomes true and
  `iterateNext()` throws `InvalidStateError`. Form-control state
  (checked/selected/value), focus and hover do not invalidate, so
  `while ((n = it.iterateNext())) n.click()` works as in browsers.
  Snapshots and single-node results are never invalidated.
- Results hold a reference on each node, so detached nodes in a result stay
  alive while the result is reachable. Attributes in results and Attr
  context nodes are identified by (namespace, local name); the Attr context
  is located after the resolver and `type` conversions run.
- `id()` searches the context node's tree including its root (a detached
  element subtree works), in one pass for all tokens; the first element per
  ID wins.
- Stack safety: operator chains (`a or b or ...`, `1+1+...`, `//a|//b|...`,
  `---x`) are flat n-ary nodes evaluated in loops, so their length is
  unbounded. Nesting (parentheses, predicates, function arguments) is
  limited to 32 levels; deeper expressions throw `SyntaxError`. Tests run
  100,000-term chains and the deepest nesting on a 256 KiB stack.
- Steps over many context nodes deduplicate as they go and, without
  predicates, stop sibling/ancestor walks at nodes already produced and skip
  contexts inside an already walked subtree, so e.g.
  `//tr/following-sibling::tr` stays linear in the number of rows.

Deviations:

- Invalidation uses a page-wide counter, so a tree mutation in any
  document of the page (not only the context node's) invalidates
  iterators.
- An attribute result is the element's `Attr` node (the object
  `getAttributeNode` returns); one whose attribute was removed since the
  evaluation reads as null.
- `node()` also matches the doctype (as upstream does; XPath 1.0's data
  model has no doctype node), so `count(/node())` is 2 for
  `<!DOCTYPE html><html>`. Its name and string-value are empty.
- The 32-level nesting cap is lower than browsers' (which have no fixed
  cap).
- The namespace axis is always empty and variable references (`$x`)
  evaluate to the empty string (the DOM API cannot bind variables).
- `string-length` and `substring` count Unicode scalar values; browsers
  count UTF-16 code units (differs only for non-BMP characters).
- Kept from upstream: a null/undefined `contextNode` in `document.evaluate`
  means the document (browsers throw TypeError); the `result` argument is
  ignored and a new `XPathResult` is always returned.
- Upstream differences, where this port follows browsers instead: upstream
  ignores namespace resolvers, matches name tests against any element after
  lowercasing, stubs `lang()`/`namespace-uri()`, never invalidates
  iterators, and throws `InvalidStateError` for type mismatches.

Tests: engine tests in `crates/lp-dom/tests/xpath.rs` (the 91-case upstream
conformance battery, axes/document order, namespaces, functions, errors,
20,000-deep trees, 100,000-term operator chains on a small stack,
multi-context steps over 3,000 rows) and unit tests in
`crates/lp-dom/src/xpath/`; JS-level tests `xpath_*` in
`crates/lp-webapi/tests/dom_js.rs` (including iterators surviving
checkbox clicks/focus), including the
Playwright `xpath=` locator shape
(`ancestor::div[contains(@class, 'h-[2.625rem]')][1]` with
`ORDERED_NODE_ITERATOR_TYPE` and `iterateNext()`).
