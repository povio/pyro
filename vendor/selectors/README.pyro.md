# selectors 0.43.0, patched

The published `selectors` 0.43.0 crate (crates.io; part of [servo/stylo](https://github.com/servo/stylo)), used
through `[patch.crates-io]` in the workspace `Cargo.toml`. License: MPL-2.0, as upstream. `.cargo_vcs_info.json`,
`.cargo-ok` and `Cargo.toml.orig` were left out of the copy. `UPSTREAM.patch` is the whole diff
(`git diff --no-index --no-prefix` of the published crate and this copy).

## The change

`matching.rs`: the featureless shadow host (CSS Scoping 1: in its own shadow tree's rules, only `:host` and the
logical pseudo-classes around it match it) matches as Blink matches it (WPT `css/selectors/featureless-006.html`,
`css/css-shadow/host-has-internal-*`, `host-multiple-004`):

- `compound_matches_featureless_host`: `:has()` is allowed in a compound that something else makes match the host
  (`:host:has(.inner)`, `:has(.inner):host`), not alone (`:has(.inner)` does not match the host). A `:not()` can
  match the host when one of its selectors can (`:not(div, :not(:host))` does, `div` not matching the featureless
  host), and none when none can (`:not(div)`, `:not(:host.host)`); it may then match other elements too, so its
  rule goes into the normal rule map as well. Upstream needed every selector of a `:not()` to match only the host
  and left the rule out of the normal map (https://github.com/w3c/csswg-drafts/issues/10179).
- `:has()` on the host looks into its shadow tree: its arguments are matched against elements that are not
  featureless, those that look at siblings (`:host:has(~ .x)`) match nothing (`descendants_only`), and
  `RelativeSelectorAnchor` matches the featureless host when the shadow root's parent link reaches it, only for the
  rules of that host's own shadow tree (`current_host`): an outer tree's `:has()` does not see into it
  (`css/selectors/invalidation/host-has-shadow-tree-element-at-*`).

`parser.rs`, `builder.rs`, `matching.rs`: `:host-context(<compound>)` (`Component::HostContext`, as Blink has it;
Gecko and WebKit do not): the shadow host of the rules' tree when it or one of its shadow-including ancestors
matches the compound (a shadow tree's first level steps to its host). Specificity and flags as `:host()`; Stylo
buckets it as universal (`selector_map.rs` matches `:host()` only). Changes of the ancestors' classes or attributes
do not invalidate it (their invalidation looks at the rules of their own trees); WPT `css/css-shadow`
`host-context-specificity-*` and `css-scoping-shadow-host-namespace` style once.
- `parser.rs` (`parse_selector`): inside `:is()`/`:where()`/`:not()` the default namespace is skipped only for the
  subject compound; a compound followed by a combinator keeps `<default ns>|*` (Selectors 4 §4.2; WPT selectors
  `is-default-ns-003`, `not-default-ns-003`).
