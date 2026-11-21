# ChaCha20-Poly1305 in Web Crypto

The candidate implements RFC 8439 authenticated encryption/decryption, 256-bit secret-key generation, raw-secret and C20P JWK import/export, wrapping, unwrapping and derivation. It uses the same ring 0.17.14 already linked by rustls. Existing symmetric algorithms accept raw-secret as their typed raw alias; existing public-key algorithms accept raw-public. Unsupported format and usage combinations follow the operation's Web Crypto checks. Neither the JSC backend nor the resolved Cargo package inventory changed.

| Direct 10 pinned URLs | Named passes | Fully passing pages |
| --- | ---: | ---: |
| LP cbdeb1193 | 0 | 0 |
| LP 73f66c8ef | 1,044 | 10 |
| Local Chrome stable 158.0.8127.37 | 1,044 | 10 |

On the wider 334 ordinary Web Crypto URLs both LP runs completed all pages. Named passes increased from 52,567 to 53,853, with 1,286 gains and zero losses. Fully passing pages rose from 220 to 232; 202 new assertion names were registered. All ten direct pages passed on LP and Chrome. The broad JSONL report retains only the published snapshot of the new Chrome census, covering 69 of 334 URLs at packaging; other Chrome cells remain null and cannot contribute an exclusive pass. The exact-158 census continues separately.

The normal controls verify the RFC 8439 ciphertext/tag vector, altered-AAD rejection, C20P round trip, AES key wrapping with ChaCha, HKDF-derived key bytes and a classic public-key format alias. All six pass on LP and signed Chrome. On a seventh check combining invalid usage and unsupported format, LP returned the draft-required SyntaxError and Chrome chose a different error. The native Web Crypto JS integration suite passed all nine tests. The original names, duplicate occurrences, page/harness statuses and each engine's missing results are retained. Source expectations were preserved.

```sh
bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$WPT" --server wptserve \
  --testdriver --http2 --generated-globals "$MANIFEST" --manifest-inventory \
  --measure-reviewed-skips --dirs non-css --jobs 4 --tests urls.json --bin "lp=$BIN" --out "$OUT"
```

[Modern Web Crypto algorithm](https://wicg.github.io/webcrypto-modern-algos/#chacha20-poly1305) · [Signed reference and full inventory](https://github.com/povio/pyro/blob/f6149854f6bd023cbfd95e0b03f75c79d509bac8/evidence/round6-chrome158/README.md)
