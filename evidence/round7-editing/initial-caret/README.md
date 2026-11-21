# Initial caret placement

On the two initial-selection-on-focus WPT variants, Pyro improved from 92/151 to 151/151. The div host gained 38 named passes and the span host gained 21. Chrome passed all 59 gains and 147/151 overall. Across 29 additional editing and selection pages, all 7,180 named outcomes held. Independent QA reproduced both WPT results and verified repeated focus, a non-editable boundary, preserved whitespace and a hidden subtree.

Frozen base `28eae5de4` uses runtime source `f15703b45` and binary SHA-256 `9acc7ae3e2bb20f4503173b437862f6a8dfb2815d95bbd4df57cd81f09d40ffe`. Candidate `bf6cebb4ff3cb38033b880350f34cedeaec2412a` has binary SHA-256 `41302d2900471ae44e141d62652150bcf3ce3297f3ead5eb173df3e482fcafe3`. Base and Chrome were measured on the focus pages. Their 29 guard pages reuse the insertion batch receipts. The final candidate ran all 31 pages and the native integration guard.

WPT revision `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Rows contain URL, subtest name, occurrence, base status, candidate status and Chrome status.

```sh
gzip -dc named.jsonl.gz | jq -sc 'map(.[0]) | unique' > urls.json
bun bench/wpt/run.mjs --wpt "$WPT" --dirs selection,editing --tests urls.json \
  --jobs 1 --bin "$LABEL=$BIN" --out "$LABEL.json"
cargo test -p pyro-webapi --test integration \
  dom_js::contenteditable_typing_deleting_and_caret_keys_match_chromium -- --exact
```

The native guard passed with one test executed and 652 filtered out, using the pinned JSC archive and the wptiter profile.
