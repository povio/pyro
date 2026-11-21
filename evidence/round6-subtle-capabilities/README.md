# Web Crypto capability queries and public-key derivation

The synchronous supports() query normalizes the real algorithm registries and checks parameter validity without generating keys or performing cryptography. The separate getPublicKey() change derives genuine public material for supported RSA, NIST EC, Ed25519 and X25519 keys, including non-extractable private keys. The browser's transient native work queue carries only owned key bytes. JS roots and promise settlement stay on the owner thread, with disposal and fork-template handling. A same-origin live sibling key is resolved through its private brand and ordinary internal key serialization. Detached sibling realms are outside the verified scope.

| Incremental comparison | URLs | Named passes before | Named passes after | Gains | Losses |
| --- | ---: | ---: | ---: | ---: | ---: |
| supports(), full ordinary Web Crypto guard | 334 | 53,853 | 54,411 | 558 | 0 |
| getPublicKey(), same guard | 334 | 54,411 | 54,473 | 62 | 0 |
| supports(), direct Window and worker pages | 4 | 0 | 550 | 550 | 0 |
| getPublicKey() and supports(), direct pages | 6 | 550 | 606 | 56 | 0 |

The full guard moved from 232 to 234 fully passing pages. All 334 pages completed on all three LP builds. Exact signed Chrome stable 158.0.8127.37 passed 616/694 named cases on the four direct pages and 682/780 on the six-page set. The broad tables explicitly retain Chrome cells missing at each packaging snapshot (204 for supports and 193 for getPublicKey); LP-versus-LP counts cover all 334. WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603, CI JavaScriptCore 0e0dd7038a7a-6f4649e395e0, macOS arm64. There were no native restarts.

Normal controls completed a cipher round trip after querying support. LP and Chrome both rejected unsupported AES IV and RSA options. For a BigInt in a numeric algorithm dictionary, LP returns false on a normalization failure as the draft specifies; Chrome throws TypeError. The additional-algorithm getter traces also differ and are retained. The final public-key tests matched Chrome on five algorithms, twenty concurrent imported-key derivations, invalid receiver behavior and a live same-origin iframe. For a supported algorithm with an invalid key type or usage, LP rejects on a crypto task after the pending microtask; Chrome rejected first in this probe. Both LP thread-state checks and nine existing native Web Crypto JS integration tests passed. Minimal embedders without a cross-thread dispatcher use their owner task queue.

Named rows retain URL, canonical name, one-based occurrence and (status, original name). Page rows retain outer, harness and strict full-page status. Column order is predecessor, candidate and exact Chrome 158. Null is unreported. The current full exact-158 census is separately versioned.

```sh
bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$WPT" --server wptserve \
  --testdriver --http2 --generated-globals "$MANIFEST" --manifest-inventory \
  --measure-reviewed-skips --dirs non-css --jobs 4 --tests urls.json --bin "lp=$BIN" --out "$OUT"
```

[Modern Web Crypto methods](https://wicg.github.io/webcrypto-modern-algos/) · [Chrome identity and cross-machine provenance](https://github.com/povio/pyro/blob/f6149854f6bd023cbfd95e0b03f75c79d509bac8/evidence/round6-chrome158/README.md)
