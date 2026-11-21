# Trusted pointer movement and boundary events

For physical mouse movement, PointerEvent.button identifies which button changed. A move with a stable buttons bitmask, pointerover/out and pointerenter/leave use −1. The corresponding MouseEvent.button remains 0. The trusted enter and leave events have composed=false and stop at the shadow boundary. Actual right-button press and release retain button=2.

## Matched measurements

| Preselected URLs | Parent / candidate / Chrome passing names | Parent / candidate / Chrome full pages | Candidate gained / lost |
|---|---|---|---|
| 4 pointer attribute variants | 64 / 204 / 204 | 0 / 0 / 0 | +140 / 0 |
| 12 ordinary pointer and mouse guards | 105 / 106 / 124 | 5 / 6 / 10 | +1 / 0 |

All four original pages reported an uncaught assertion on pointerleave.button and an error harness status. On the candidate all four complete with an OK harness. The corrected event assertions allow WPT to register 140 additional subtests. All 140 pass on the candidate and Chrome. One existing nested-frame assertion per page still fails on both engines in the pinned driver. On the guards the additional passing test is “Attributes of boundary events during dragging mouse.” All 12 guard harnesses completed on both LP binaries; zero missing names, native restarts or watchdog terminations.

The controls use the same pinned testdriver on both engines. It translates the pen variants’ actions to Playwright mouse input. Those results verify the behavior of the actual mouse path. Physical pen hardware and chorded-button transition semantics remain outside this implementation.

A separate real-input control moves across a shadow target, moves with the right button held and releases it. Each raw row records event type, button, buttons, composed and isTrusted. On the ten rows delivered by both LP builds, six parent rows and all ten candidate rows agree with Chrome. Chrome additionally delivered pointerenter, mouseenter, pointerleave and mouseleave to the internal target; both LP builds omitted all four in this shadow-tree case. The control retains these missing events.

## Source, profiles and commands

- Parent 8e47c7148ea89dc8646c7e0b5efe48e4bb97aab5, browser SHA-256 88fd20729b7843c85f3f78dd55017800bffa7761218190d56ba9ed7fd1873cf7. Candidate 7f0a6d4279f8e4c2a38f3f96974a2a8103fb5c98, binary SHA-256 8e73550b1294f61404db8f9568024dcaa2f89dd4bc3a0b826facecf413126712. Both use pinned CI JSC 0e0dd7038a7a-6f4649e395e0 and wptiter. Independent source review includes mouse, pointer, touch, click, chord and drag behavior.
- WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603, signed macOS arm64 Google Chrome stable 158.0.8127.37. Both LP columns are fresh matched one-worker runs. The Chrome WPT column reuses 16 exact-revision frozen raw rows and includes their contributing source shard checksums. For the separate shadow control all three columns are fresh.
- Named keys include URL, original name and occurrence; page rows retain lifecycle, harness outcome, whole-page status and elapsed milliseconds. Full pass requires complete/OK and all reported names passing. Parent and candidate use fresh contexts, wptserve, HTTP/2 and testdriver.

```sh
bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$WPT" --server wptserve \
  --testdriver --http2 --generated-globals "$MANIFEST" --manifest-inventory \
  --measure-reviewed-skips --dirs non-css --jobs 1 --slow-jobs 0 \
  --tests selection.json --bin "lp=$BIN" --out "$LABEL.json"
python3 report.py
PLAYWRIGHT_CORE="$PW_CORE" CHROME_BIN="$PINNED_CHROME" node controls.cjs "$LABEL" "$BIN_OR_CHROME"
```

## Offline harness ranking

The starting census is fixed at 1f1be032436bfeee9f48cdb5acbc0f71d21d5af9. The ranking joins its published named pass sets and page dispositions to saved LP raw harness messages; it performs no browser runs. It searches an explicit list of ordinary feature areas, discards held or missing rows, requires a completed OK Chrome harness, and excludes editing, CSS, security-oriented and fault-related paths. Of 1,313 selected exceptional LP page rows, 1,308 contributed to the raw-message ranking after an additional message-content filter. Blank messages are grouped as unknown causes within each area. The ranking is a triage subset, separate from the full census. Similar exception text, especially a generic constructor or setup exception, does not prove a single implementation cause.

Among attributable messages, missing EncodedVideoChunk accounted for 37 pages and 366 Chrome-only names, absent speculation-rules script support for 236 and 300, and missing AudioDecoder for 26 and 262. Ending such a harness error does not by itself implement the decoding or asynchronous work that follows. The specific trusted pointer assertion was shared by four variants and accounted for 140 Chrome-only names. The included ranking script lists all filters and groups, and the byte-preserving compressed output includes the underlying exceptional page rows and name-count boundaries.
