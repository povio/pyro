# Whitespace insertion

Candidate `f15703b4538ad685c131cfcb209025adcaef88c7` gained 186 passes and lost zero on 7,180 matched named checks across 29 WPT pages. The Chrome compatibility insertion page went from 43/223 to 223/223. The three rich-editing insertText shards gained four, one and one passes respectively. The deletion and selection guards retained their passes, as did the nowrap case that failed on the first insertion candidate.

The frozen base is `7b63c4787`. Its runtime source matches `dd5572211`, whose binary and original 26-page result were reused. The three extra typing pages were measured on the base and on stable Chrome. All 29 pages were run on the final candidate. Base binary SHA-256 was `05c968e261ccf373dc1114c3fac4674790f1e3cfaf56b75cfd8fad3cbd568d9d` and the candidate binary was `9acc7ae3e2bb20f4503173b437862f6a8dfb2815d95bbd4df57cd81f09d40ffe`.

WPT revision `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Rows contain URL, subtest name, occurrence, base status, candidate status and Chrome status.

```sh
gzip -dc named.jsonl.gz | jq -sc 'map(.[0]) | unique' > urls.json
bun bench/wpt/run.mjs --wpt "$WPT" --dirs editing,selection --tests urls.json \
  --jobs 1 --bin "$LABEL=$BIN" --out "$LABEL.json"
```
