# ML-KEM provider feasibility on macOS arm64

**Exact Chrome stable 158.0.8127.37 supports ML-KEM-768 and ML-KEM-1024 in this profile.** For each, a generated key pair, native WebCrypto encapsulation and decapsulation produced the same 32-byte secret. Ciphertexts were 1,088 and 1,568 bytes. The same generation request for ML-KEM-512 rejected with NotSupportedError. Keys and secrets were kept in process. [Browser round trips](results/chrome-roundtrip.json)

At pinned WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603, both the Window and dedicated-worker successful-keygen pages reported 468 assertions. Chrome passed 312 each, all at 768/1024. LP b57c35dd45d54d6781aafcb2c8242c9ad9b26e04 passed zero each. All four harnesses finished OK and neither browser fully passed either page. Chrome used the verified WPT-profile launcher with secure WPT origins, current feature flags and ordinary GPU initialization. The LP binary SHA-256 was 296b3d4f9775c92d98d3bd7aa898efc68b1a18e90e591af5a428ad8decd4e9bf. [LP raw](results/base.json) · [Chrome raw](results/chrome.json) · [Reference identity](results/chrome-identity.json)

## Maintained providers tested

Each timing is the median of 31 normal independent round trips after one warmup, in microseconds. Every case generated an independent key, encapsulated via its public key and confirmed agreement through decapsulation. These are native provider costs, measured on one macOS 27.2.1 arm64 system, not browser latency. FIPS 203 algorithm support does not establish a validated FIPS operating mode.

| Provider | Parameter set | Key generation | Encapsulate | Decapsulate |
| --- | --- | ---: | ---: | ---: |
| OpenSSL default provider 3.6.3 | 512 / 768 / 1024 | 29 / 42 / 63 | 22 / 28 / 38 | 33 / 43 / 58 |
| Apple CryptoKit | 768 / 1024 | 141 / 217 | 16 / 23 | 65 / 96 |
| RustCrypto ml-kem 0.2.3 | 512 / 768 / 1024 | 22 / 36 / 55 | 20 / 33 / 51 | 25 / 39 / 57 |

OpenSSL's 35,720-byte executable was 18,880 bytes larger than a minimal C executable using the same compiler flags. Its external runtime was Homebrew libcrypto.3.dylib (4,846,032 bytes, installed as OpenSSL 3.6.3). The Homebrew archive was 8,575,136 bytes; that is not a linked binary delta. Inspection of the dylib's LC_BUILD_VERSION reported a **macOS 26 minimum**, despite setting the probe target to macOS 14. The system-supplied openssl command was LibreSSL 3.3.6. The probe demonstrated standard EVP generation, raw public export/reimport, 64-byte retained seed availability, and provider DER encoding sizes. The normal LP build does not ship this library. [Probe](provider.c) · [Raw results](results/provider.jsonl)

Apple CryptoKit's SDK declares MLKEM768 and MLKEM1024 for macOS 26 and later. It exposes native 64-byte seed serialization for those two levels. The test's availability check guards execution on older systems; the macOS 14 product floor does not acquire a KEM backend there. The framework is Apple-specific and supplies no ML-KEM-512 type in this SDK. [Probe](apple.swift) · [Raw results](results/apple-results.jsonl)

RustCrypto 0.3.4 declares Rust 1.87, above the workspace's 1.85 minimum. Older 0.2.3 declares 1.74. Its separately resolved graph (19 packages including the probe and target-dependent entries) declared no dependency MSRV above 1.85; zeroize 1.9.2 declared 1.85. We actually compiled and ran with Rust 1.100.0, not Rust 1.85. With release thin LTO and stripping, the standalone RustCrypto harness was 472,608 bytes versus 339,504 for the minimal Rust control, a 133,104-byte difference. This is not a whole-browser delta. Both published RustCrypto packages explicitly warn the implementation has **never been independently audited**; 0.2.3 is an older release and continued maintenance of that line was not established. Its default secret encoding is an expanded key and cannot be labeled raw-seed. No RustCrypto KEM dependency was added to LP. [Crate metadata](results/rustcrypto-02.txt) · [Raw results](results/rustcrypto-results.jsonl) · [Upstream documentation](https://docs.rs/ml-kem/0.2.3/ml_kem/)

## Replay

Set OPENSSL_PREFIX to an OpenSSL 3.6 prefix. Use the matching platform compiler, CryptoKit-capable SDK, and appropriately licensed provider installations. The following commands operate on disposable local keys. No secret or key material is printed or saved.

```sh
xcrun clang -O2 -mmacosx-version-min=14.0 -I"$OPENSSL_PREFIX/include" provider.c "$OPENSSL_PREFIX/lib/libcrypto.3.dylib" -o provider
./provider
xcrun clang -O2 -mmacosx-version-min=14.0 control.c -o control
xcrun vtool -show-build "$OPENSSL_PREFIX/lib/libcrypto.3.dylib"
xcrun swiftc -O -target arm64-apple-macosx14 apple.swift -o apple
./apple
MACOSX_DEPLOYMENT_TARGET=14.0 cargo build --manifest-path rustcrypto/Cargo.toml --release --locked
rustcrypto/target/release/mlkem-provider-probe
```

For WPT, set CENSUS_ROOT to the exact 158 census runner/launcher directory, WPT_CHECKOUT to the pinned checkout, and WPT_MANIFEST to its corresponding manifest. Run `./run-wpt chrome "$CENSUS_ROOT/launch-158"` and `./run-wpt lp "$LP_BASE"`. For the one-window operation probe, set NODE_PATH to Playwright Core 1.65.1 and run `bun roundtrip.cjs "$CENSUS_ROOT/launch-158"`. The launch script and chosen URL list are included for reference. [WPT commands](run-wpt) · [Chosen URLs](urls.json)
