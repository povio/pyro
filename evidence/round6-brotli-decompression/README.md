# Brotli DecompressionStream

LP can decode RFC 7932 Brotli using the streaming API already linked by pyro-net. The new pyro-webapi dependency edge selects the identical locked brotli-decompressor 6.1.0. The dependency package and feature selection stay the same and the browser has no Brotli encoder. The decoder consumes split input and drains full output buffers, releases the history window at the end marker, rejects truncation and rejects trailing bytes in the same or a later write. The shared transform now distinguishes invalid compressed data from a successfully decoded stream with a suffix. In the latter case the waiting reader can receive decoded bytes before the TypeError. [Compression Standard](https://compression.spec.whatwg.org/)

The 66 selected non-CSS URLs use WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603. Both LP binaries used the CI JSC 0e0dd7038a7a-6f4649e395e0. LP control cdc2323a2db447c5d392b7c87449d43b1995c7e4 and feature 38ae18a952f1dc8dcea93266928725d57290d5fb ran with four jobs. The comparison reuses macOS arm64 Chrome stable 157.0.8103.80 results from the matched WPT-profile census. The selection includes the window, dedicated worker, shared worker and service worker variants present in the pinned manifest. Large-window extension, large-flush-output and corrupt-input fixture groups were excluded from this ordinary guard. [Source](https://github.com/povio/pyro/tree/38ae18a952f1dc8dcea93266928725d57290d5fb) · [Census and Chrome provenance](https://github.com/povio/pyro/tree/57a9450abdaa792dafae09f6e84d4236fa72a2ea/evidence/round6-noncss-census)

The records retain every selected URL and original subtest name plus duplicate occurrence. Null indicates an unreported observation. Each result has LP control, LP candidate and Chrome status in order; page records also preserve harness status and fully passing flags. Count suffixes 0, 1 and 2 refer to these engines in the same order.

The normal workload uses 6,500 sample transaction records totaling 359,996 bytes (17,930 compressed). Node's Brotli encoder generates the fixture outside the browser. LP reproduces the entire text at chunk widths of 1, 7, 1,024, 16,384 and 17,930 bytes. A separate progressive read returns 175,291 bytes before the producer sends the final half of compressed input. Chrome stable rejects the Brotli constructor on the same workload. Four native codec tests passed, including single-byte splits, truncated final input and same/later-write trailing bytes.

```sh
bun bench/wpt/run.mjs --wpt "$WPT" --kind testharness --dirs non-css --jobs 4 \
  --server wptserve --testdriver --http2 --generated-globals "$MANIFEST" \
  --manifest-inventory --measure-reviewed-skips --tests urls.json --bin "lp=$BIN" --out "$OUT"
cargo test --locked --offline --profile wptiter -p pyro-webapi --lib compression::tests
cargo tree --locked --offline -p pyro-webapi -i brotli-decompressor@6.1.0
```

Named passes increased from 866 to 1,034 (+168/−0, zero new or missing names). Fully passing URLs increased from 17 to 41. Chrome passed 904 names and 18 full URLs: 156 names passed only on final LP and 26 only on Chrome. Two LP harness errors remained on the unchanged buffersource window and service-worker page results; Chrome completed all pages with OK harness status. All selected URLs were observed. The encoder and the experimental large-window extension remain unsupported.
