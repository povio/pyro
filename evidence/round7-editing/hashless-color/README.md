# Applied hashless foreColor values

Frozen source `bab5f5dc7907f15a3a06c7d4dfea0a3755ae1854`, tree `bf60266202d7910ce266c0f52057eca527e8c74f`, candidate executable SHA-256 `9ea60343757ada406d351e7774e08f912c19f0b3de69a4022c1ee6576098a753`. Exact parent `9bc2676df693fdb63ac7e9a39f99e2b9a20825fb` was built because the archived editing executable differed in shipped Web Audio code. Signed WPT rows retain their Chrome 158.0.8127.37 provenance. Standards and quirks direct probes were run against signed Chrome, exact parent and both candidates.

The foreground editing command recognizes literal three- and six-digit hex colors and otherwise uses Stylo’s hashless numeric-token parser. The parser applies in both document modes. Surrounding whitespace matters to the literal fast path. Parsing of CSS-wide keywords, ordinary CSS colors, background-color commands and color controls remains separate.

| Input | Signed and candidate selected and typed color |
|---|---|
| `000000fff` | `#000fff` |
| `123` | `#112233` |
| `  123  ` | `#000123` |
| `0af` | `#00aaff` |
| ` 0af ` | `#0000af` |

## Named WPT results

Across 44 pages there were **zero gains and two losses** on matched pinned assertions, with unchanged membership. Both losses are counted. The pinned fixture expects unchanged markup and a black queried color for `000000fff`. Chrome and the candidate both wrap the selected text in FONT color="#000fff" and report rgb(0, 15, 255). [Exact diagnostics](pinned-diagnostics.json)

## Direct controls

The standards and quirks suites each contain 144 selections and carets. First-command glyph arrays match in 138/144 in each, and post-typing arrays in 132/144. The 96 supplemental token cases match in 94 after the command and 92 after typing. All 24 padded quirks cases match after the command, typing and undo. The 46 initial event regressions (16 standards, 16 quirks, 14 token cases) were all recovered individually. A hashless application to a selected range now emits the signed input event with empty inputType and null data. The independent per-case results and raw snapshots record selection, original connectivity, events and undo. Remaining keyword, alpha-color, wrapper serialization and ordinary typing-event differences are recorded separately from the recovered hashless effects.

Four newly differing post-typing endpoint paths are counted in the separate 36-case spanning and neighboring-command suite. They cover `000000fff` and space-padded `123` in both formatting modes. The original parent fails to color the typed Q but places it at [0,0]:1. The candidate gives Q the correct color at [0,0,0]:1. Chrome also applies that color and removes the redundant child spans, leaving Q at [0,0]:1. Existing valid #00aaff and red controls exhibit the same redundant span structure on both Pyro arms. [Four cases and valid-color comparators](spanning-caret-followup.json.gz)

All 24 unsupported-host controls retained their parent behavior. The native color-well test passed. The exact parent and candidate agreed on all 10,092 command multitest and 13,060 static neighboring statuses.
