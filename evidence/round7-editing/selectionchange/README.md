# Text-control selectionchange

Candidate `d25213606597c62ffb6fa2e029a436e34e5b9323` on `18ab33869`. WPT revision `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`.

Five selection WPT pages improved from 30/68 to 68/68. Stable Chrome passed the same 68. Five editing guard pages retained all 2,741 named outcomes. The four separate rich, plaintext and nested formatting probes passed on the candidate.

The compressed JSON Lines rows contain URL, subtest, occurrence, base status, candidate status and Chrome status. Use the URLs from the rows to reproduce all ten WPT pages.

```sh
gzip -dc named.jsonl.gz | jq -sc 'map(.[0]) | unique' > urls.json
bun bench/wpt/run.mjs --wpt "$WPT" --dirs selection,editing --tests urls.json \
  --jobs 1 --bin "$LABEL=$BIN" --out "$LABEL.json"
```
