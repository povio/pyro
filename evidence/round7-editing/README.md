# Plaintext-only editing commands

Both variants of the styling commands WPT improved from 90/150 to 150/150, matching Chrome. The rich-editable bold, createLink, and ordered-list controls retained all 2,441 named outcomes. Independent QA also checked bold across rich, plaintext and nested hosts, improving from 2/4 to 4/4.

Frozen base `305840f45`, candidate `c30735fc5`, WPT `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`. Each row in `named.jsonl.gz` contains the URL, subtest name, occurrence number, base status, candidate status and local stable Chrome status. All three runs completed without a browser restart.

Run once per binary with the local stable Chrome CDP wrapper for the reference.

```sh
bun bench/wpt/run.mjs --wpt "$WPT" --dirs editing \
  --match 'plaintext-only/styling-commands|run/(bold|insertorderedlist)\.html\?1-1000|run/createlink\.html$' \
  --jobs 1 --bin "$LABEL=$BIN" --out "$LABEL.json"
```
