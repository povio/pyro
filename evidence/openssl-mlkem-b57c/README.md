# Experimental OpenSSL ML-KEM in WebCrypto

The opt-in LP binary passed **936/936 named tests** across the two unmodified successful-keygen pages, up from 0/936 on its production parent. Both pages now pass in full. Chrome stable 158.0.8127.37 passed 624/936. The remaining 312 Chrome failures were for ML-KEM-512. This is a two-page comparison, outside the production integration score. [Source](https://github.com/povio/pyro/commit/f1a3e2aa115cadb02c570502a11b45fcfbeb817d) · [Named and strict-page pass sets](results/named-and-page-sets.json.gz)

| Same window and dedicated-worker URLs | Passing subtests | Fully passing pages | Harness errors/timeouts |
| --- | ---: | ---: | ---: |
| Production LP b57c35dd4 | 0/936 | 0/2 | 0 |
| Rebuilt experimental source with feature disabled | 0/936 | 0/2 | 0 |
| Experimental LP with external OpenSSL | 936/936 | 2/2 | 0 |
| Chrome stable 158.0.8127.37 | 624/936 | 0/2 | 0 |

The LP change gained 936 named assertions, lost zero, and added or omitted zero names. Eight additional ordinary AES-GCM, HMAC, Ed25519 and X25519 generation pages retained every one of their 1,096 passing assertions. No test expectations changed. [Production](results/base.json) · [Feature enabled](results/reviewed.json) · [Feature disabled](results/default-topic.json) · [Chrome](results/chrome.json) · [Regression guard](results/guard-reviewed.json)

## Backend and browser behavior

The experimental adapter uses OpenSSL’s maintained default provider. Its high-level EVP operations perform key generation, encapsulation, decapsulation, public-key checking, seeded reconstruction and key encoding. Each operation creates an isolated library context. It requests canonical seed-only private encoding in that context and leaves the browser TLS stack alone. The browser uses its existing CryptoKey slots for algorithm, typed key material, extractability, usages and structured cloning. Native temporary private and shared-secret byte buffers are cleansed on release. The JavaScript key material follows existing CryptoKey lifetime and garbage collection conventions.

All 15 supplemental browser checks passed. They cover three normal key agreements, three format round trips, ordinary AES-GCM message encryption/decryption with three pairs of independently derived nonextractable shared keys, cross-worker structured-clone agreement at each parameter level, and three public upstream known-answer decapsulation vectors. A separate secure-context check found the experimental encapsulate/decapsulate methods on secure Window and dedicated Worker only. A feature-disabled build omitted the methods in both origins. [Browser controls](results/probes-reviewed.json) · [Feature-on exposure](results/secure-reviewed.json) · [Feature-off exposure](results/secure-off.json)

Raw public keys are 800, 1,184 and 1,568 bytes. Raw private seeds are 64 bytes. OpenSSL generated SPKIs of 822, 1,206 and 1,590 bytes and seed-only PKCS8 encodings of 86 bytes for every level. The bridge imported and re-exported these canonical forms with exact byte equality. Valid expanded and combined seed/expanded PKCS8 encodings are recognized and rejected as unsupported. JWK, hybrid KEMs, getPublicKey, and general PKCS8 compatibility, including optional attributes, are outside the implementation. [Codec observations](results/bridge-results.jsonl) · [Alternate-format classification](results/alternate-formats.jsonl)

The verified local Chrome profile consumed the normal API in a secure WPT document. ML-KEM-768 and ML-KEM-1024 returned matching 32-byte shared secrets, and 1,088- and 1,568-byte ciphertexts, respectively. ML-KEM-512 generation rejected with NotSupportedError. The exact WPT revision was `a421f2fea5eb7f91e0db399e44b0fb0779ee1603`, using wptserve, testdriver, native HTTP/2, the manifest-first no-native-retry runner, four workers and fresh contexts. [Reference identity](results/chrome-identity.json) · [Chrome operation observations](results/chrome-roundtrip.json)

## Packaging and measured runtime

The feature-enabled experimental executable links a separate `libcrypto.3.dylib`. The feature-disabled binary does not. In two local experimental-source builds with identical wptiter settings, stripping the binaries for size inspection gave 98,046,464 bytes enabled and 98,026,576 disabled: **+19,888 executable bytes**, or **+5,750 bytes after gzip-6**. The separately required Homebrew dylib adds **4,846,032 bytes** (2,296,761 compressed). These are executable/library observations, not published npm archive sizes. [Sizes and hashes](results/package.json)

The verified Homebrew OpenSSL 3.6.3 dylib was built for macOS 26.0. It cannot establish support for LP’s macOS 14 floor or Linux. Nothing in the default package links or bundles it. The only lockfile difference is the experimental build-dependency edge to the workspace's existing cc package. Redistributable, floor-compatible provider binaries, platform CI, provider lifecycle/performance work and broader interoperability review would be required before proposing integration. The experiment does not claim a validated FIPS operating mode. [Standalone OpenSSL, CryptoKit and RustCrypto comparison](https://github.com/povio/pyro/blob/d9abc3821ab2386cd16b5def29c92d978cf96b83/evidence/mlkem-provider-b57c/README.md)

Actual secure browser batch timings included provider loading, key conversion, binding and promise costs. One warmup preceded five batches of 32 independent exchanges for each level. Each reported value is the median of five per-operation batch means.

| Level | Generate | Encapsulate | Decapsulate and compare |
| --- | ---: | ---: | ---: |
| 512 | 0.931 ms | 0.950 ms | 1.125 ms |
| 768 | 1.200 ms | 0.962 ms | 1.094 ms |
| 1024 | 1.041 ms | 0.819 ms | 0.969 ms |

These local observations are not a browser-to-browser performance comparison. The standalone OpenSSL EVP probe used an already-loaded provider and observed much lower small-operation times. No secret values were output from either timing harness. [Browser samples](results/browser-timings.json)

## Review and reproduction

Independent source review identified an error classification mismatch. The revised adapter uses OpenSSL ASN.1 decoding and algorithm-identifier checks to distinguish recognized unsupported private forms from malformed DER. All six normal alternate-encoding samples produced the expected unsupported status; canonical seed/public cases remained green. Source review cleared the bounded experiment with the limitations above. Source and evidence are on separate experimental branches. [Provenance](results/provenance.json)

Build the source commit with a matching JSC archive and an explicit OpenSSL 3.6+ development prefix. The measured feature-on artifact SHA-256 is `ebc58c2cd76bdb81bbaaaeeea0638c10aedea5d9be285983578a139ab05c627f`. The local build scheduler supplied the following settings.

```sh
export CARGO_PROFILE_WPTITER_INHERITS=release-fast CARGO_PROFILE_WPTITER_LTO=false
export CARGO_PROFILE_WPTITER_CODEGEN_UNITS=16 CARGO_PROFILE_WPTITER_INCREMENTAL=true
export PYRO_JSC_DIR=/path/to/0e0dd7038a7a-6f4649e395e0
export PYRO_MLKEM_OPENSSL_PREFIX=/path/to/openssl-3.6
cargo build --profile wptiter --features render,pyro-webapi/experimental-mlkem-openssl
```

Preserve that binary; then rebuild with `--features render` to verify the default paths and inspect both with `otool -L`. The accepted production parent executable SHA-256 is `296b3d4f9775c92d98d3bd7aa898efc68b1a18e90e591af5a428ad8decd4e9bf`.

Set `SOURCE_REPO`, an empty `RUNNER_ROOT`, and `CHROME_APP` to the verified signed Chrome 158.0.8127.37 application. Run `./prepare-runner.sh`. Set `WPT_CHECKOUT`, `WPT_MANIFEST`, `NODE_PATH` (Playwright Core 1.65.1), `CHROME_LAUNCHER="$RUNNER_ROOT/census/launch-158"`, `LP_BASE`, `LP_EXPERIMENT`, and `LP_FEATURE_OFF`. Run `./run.sh`. Run `bun source/browser-bench.cjs "$LP_EXPERIMENT"` to repeat the timing check. [Runner preparation](prepare-runner.sh) · [Matched WPT and controls](run.sh)
