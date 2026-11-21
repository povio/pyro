## Reproducing a reviewed reference selection

The archived runnable selections are enumerated in plans/tasks.json and plans/shard-selections.json.gz. Materialized selection files were immutable throughout measurement. Use only an approved selection with the verified stable app, current non-replay runner and transient private profile on this host. A browser resource termination is an observed interruption; keep partial rows and resolve provenance without rerunning a known fault. The runner does not schedule an automatic retry of native failures.

From the original measurement cache layout, the per-shard command was:

    bun bench/wpt/run-no-native-retry.mjs --kind testharness --wpt "$WPT" \
      --server wptserve --testdriver --http2 --generated-globals "$MANIFEST" \
      --manifest-inventory --measure-reviewed-skips --dirs "$SCOPE" --jobs 4 \
      --tests "$SELECTION" --bin "chrome158=$REFERENCE_LAUNCHER" --out "$NEW_RESULT"

The manifest is the official root manifest SHA-256 34a67512a84d43630f7adbecebd4378df5f410446591015cc6a20a3034e02819. The WPT tree is a421f2fea5eb7f91e0db399e44b0fb0779ee1603. SCOPE is non-css or css as listed for each shard. The browser launcher uses the isolated reference cache's Google Chrome.app, not the system installation. Use the exact signed archived build captured in reference-identity.json. A vendor URL that serves the current release may now refer to a different build. The runner source is identified by its SHA-256; its existing regular and non-replay source provenance is in the frozen non-CSS census. The private Playwright package artifact is https://registry.npmjs.org/playwright-core/-/playwright-core-1.65.1.tgz.

Studio's Git-tracked source records are identified by immutable commit, blob and compressed-byte hash in studio-compatibility.json. Studio's browser argument list and number of Chrome jobs per shard are not fully attested by those source records; those unknowns remain separate from the verified 158 version and WPT pin. No unobserved Chrome outcome is replaced by another product, release channel or LP result.
