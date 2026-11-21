# Whitespace after Backspace and Delete

On 26 editing and selection WPT pages, passes rose from 3,057 to 3,759 among 4,171 matched named subtests. Chrome passed all 702 gains. There were zero lost passes or changes to subtest membership. Each of the two main whitespace deletion pages gained 350 passes. The first rich editing delete and forwarddelete shards each gained one. The separate rich, plaintext, nested formatting and selection-event probes passed 6/6.

| Browser | Source | Binary SHA-256 |
|---|---|---|
| Pyro base | a0376f10b5aa3a22314973a7f3f2af3ea1ad5ac2 | 78df9698f76ea6a4799470e0c439162d8a0e4765cbf7bb0516a3c80c97a185b8 |
| Pyro candidate | dd5572211e6699a3200ae0e95ab3b38526064243 | 05c968e261ccf373dc1114c3fac4674790f1e3cfaf56b75cfd8fad3cbd568d9d |

Both builds used clean trees, wptiter with render, LTO disabled, 16 codegen units, incremental compilation and JSC archive SHA-256 `3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029`. The exact base and local stable Chrome runs from the first comparison were reused when a text-node boundary adjustment restored two intermediate losses. The final candidate was rerun on every selected URL.

WPT revision `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Each row contains URL, name, occurrence number, base status, candidate status and Chrome status. The static server uses isolated ports and a single WPT worker.

```sh
gzip -dc named.jsonl.gz | jq -sc 'map(.[0]) | unique' > urls.json
bun bench/wpt/run.mjs --wpt "$WPT" --dirs editing,selection --tests urls.json \
  --jobs 1 --bin "$LABEL=$BIN" --out "$LABEL.json"
```
