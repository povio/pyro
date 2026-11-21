# Dashed function calls: pinned compatibility experiment only

**Do not integrate or count these as standards gains.** On the two reported pages the candidate matches 16 historical Chrome 158 passes. None of the 16 establishes an independent, unambiguous standards fix. In particular, rejecting empty arguments changes syntax expressly permitted by the November 4, 2026 CSS Mixins editor’s draft. This experiment addresses only parse-time acceptance, not named parameter binding, argument substitution, or the two separately tracked parameter-scoping losses.

## Assertion-by-assertion disposition

The table gives the literal contents of `--func(...)` in every recovered assertion (spaces inside code spans are significant). Every assertion expects assigning the call to `style.top` to leave the property unset. `recoveries.tsv` preserves the exact WPT names and paths.

| Argument text | Count | Disposition |
| --- | ---: | --- |
| `,`, ` ,`, `, `, ` , ` | 4 | Direct conflict: a single comma can pass one empty argument; outer whitespace does not change that. |
| `a,,b`, `a, ,b` | 2 | Direct conflict: an empty positional argument is allowed between other arguments. |
| `asdf,` | 1 | Direct conflict: a final comma is allowed and the final empty argument is dropped. |
| `{}`, `1, {}`, `1, { }` | 3 | Likely conflict by composition, not an explicit example: CSS Values treats the braces as delimiters, not the argument value; Mixins permits empty values and trims whitespace. CSS Syntax defines a present declaration-value as one or more tokens, so the interaction with an optional production inside an empty wrapper merits explicit clarification. Do not justify a rejection based on the comparator alone. |
| `--myident:`, `--myident: `, ` --myident:`, `--myident :`, ` --myident : `, `10px, --myident : ` | 6 | Tentative reservation only. The dated call grammar has no restriction on a leading dashed identifier followed by a colon. An unwrapped colon is not generally excluded from a declaration-value. Rejecting these calls at parse time is not established by the draft. The check can be mechanically separated from empty-argument rejection, but that does not make it independently normative. |

Sources: [CSS Mixins §3](https://drafts.csswg.org/css-mixins-1/#using-custom-functions), [CSS Values 5 free-form productions](https://drafts.csswg.org/css-values-5/#free-form-productions), [CSS Syntax declaration-value](https://drafts.csswg.org/css-syntax-3/#typedef-declaration-value), [CSSWG issue 11749](https://github.com/w3c/csswg-drafts/issues/11749) (open when checked November 21; October 16 comment proposes reserving top-level colons, not an adopted grammar). The tentative page also accepts a colon later in an argument and a wrapped prefix; the candidate is not a general top-level colon ban. A draft-aligned implementation should test acceptance and computed argument/default behavior separately, not infer either from Chrome’s rejection.

## Attribution and reproducibility

- Original independent frozen/integrated receipts: `7c4f42242:docs/validation/studio-integrated-4126821de`. The pre-feature parent `ba53808e` passes the 16 rejection assertions but also rejects valid controls. Source change `50a11942` introduced the positional `FunctionCall::parse` without rejecting these inputs. This exact commit did **not** build (six Rust errors; see `logs/first-build.log.gz`). Its direct child `90623d76` is the first runnable bad revision; the invocation parser is unchanged. `pre.json.gz` and `first.json.gz` are two-page historical runs. The first integration merge containing the source change was `783b243c`.
- The 50-page cohort combines the 27 CSS Values evidence URLs with the function pages and parameter-scoping control, deduplicated in `urls.json`. `baseline` = `0ded6d51`, `integrated` = `87fc7c1b`, `candidate` = `e66f8889` on that parent. Two opposite-order integrated/candidate runs agree exactly. As of the compatibility check, later tip `2cefb327` differs only in editing code/evidence; merge-tree is clean. No runtime or recovery claim for that later tip.
- All 50 harnesses completed OK. Integrated → candidate: 1,132 → 1,148 named passes of 1,253; 24 → 26 all-pass pages. Exactly 16 Fail → Pass, 1,237 unchanged. Parsing: 42/52 → 52/52; tentative named-argument page: 9/15 → 15/15. The 15 named default controls remain passing. Both separately tracked scoping losses are unchanged. No full-suite run or extrapolation.
- Same tracked-clean WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, runner tree `7efc9828957ae18dbd8c8c7df59cba3c59524633`, fresh contexts, isolated Cargo target and at most four build jobs. `chrome.json.gz` is a subset of the archived Chrome stable 158.0.8127.37 comparator, **not** a fresh run. Full compressed reference SHA-256: `7858861e3a061f3d352b47336de17ba0dc06a89f81ceb7b48e480ce94e85a4e0` (`refs/outcomes-chrome-main.json.gz` under the original evidence directory).

Binary SHA-256, in historical parent, first runnable, baseline, integrated, candidate order:

```
40975f5319b3551dbc4a80b16528d3285c03efb44b736eca597166527aedc006
732d75665d1c5a96f83a836ec20f7776a8a20c51fb4beaa61d6cd0ebd853eec1
ae17f5dfb6989257dc34e46a70c90046a00afea22b068735bb62fc1e3cae57f7
bd8ca27d31874024e379b2457bc194c3a87b5f023a2795fec33a43e330c31e10
28b116d4ebb1ae57d23242f49d88829acb8f7ae1cded1adb5c6ddc493af542c6
```

From the repo root, check all named outcomes with `python3 docs/validation/round7-dashed-functions/verify.py`. To reproduce one leg after building its revision using `pyro-build --features pyro/render` in an isolated target, use the matching runner and WPT checkout:

```sh
bun bench/wpt/run.mjs --kind testharness --dirs css --server wptserve --testdriver --jobs 2 \
  --tests docs/validation/round7-dashed-functions/urls.json --wpt "$WPT_DIR" \
  --bin "subject=$PYRO_BIN" --out "$OUT"
```
