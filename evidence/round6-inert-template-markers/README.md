# Parser-time inline template marker replacements

LP base `cbdeb11931f74c3de4417a25fb9103f1df84d54e`, candidate `8a720bd75783f5c7e03d32464b75db10758e940a`, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, signed macOS arm64 Chrome stable 158.0.8127.37. Pinned JSC cache key `0e0dd7038a7a-6f4649e395e0`. Base binary SHA-256 `4095893f0cf7ad675427ab8d1489dbad2a9232ea935fa8d0431da1485963d6b3`, candidate `99669afa34705ce61d18c94ad7813b7ee3e9a7b51b3a265e05ee7769638bd340`.

| Selection | Base names | Candidate names | Chrome names | Base / candidate / Chrome full pages |
|---|---:|---:|---:|---|
| 32 template-for pages | 62/494 | 138/494 | 475/494 | 5 / 7 / 30 |
| 33 parser and DOM controls | 778/801 | 778/801 | unmeasured here | 27 / 27 / — |

All 76 newly passing names pass in Chrome. Both selections had zero losses, missing names, timeouts and restarts. Ten filtered Rust tests passed, including explicit scope, EOF, Range, editing and external-template regressions. The candidate supports detached, unfiltered HTML innerHTML/outerHTML and adjacent fragment insertion with inline patches. Live document parsing, script-activating fragment parsing and network-backed template patches remain open. These selections establish no suite-wide delta.

The raw compressed JSON files include runner configuration, page status, subtests, exclusions and exact selection. In the derived named tables each row holds URL, name, occurrence, then per-browser statuses in base, candidate and (focus only) Chrome order. Full pages require a completed OK harness, at least one subtest and every subtest passing.

Matched measurements used the following runner invocation for each binary and selection.

```sh
bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$WPT" \
  --server wptserve --testdriver --http2 --generated-globals "$MANIFEST" \
  --manifest-inventory --measure-reviewed-skips --dirs non-css --jobs 4 \
  --tests "$SELECTION" --bin "$LABEL=$BINARY" --out "$LABEL.json"
```
