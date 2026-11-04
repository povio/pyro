# Release packaging

Release assets contain a render-enabled `lightningpanda`, the matching
`lp-snapshot-compare`, and one identical `lightning-panda` payload. Bump the
workspace and client versions together before building. Record the source commit,
input hashes and validation results; packaging alone is not functional testing.

Publish these six assets for a release version `X.Y.Z`:

- `lightningpanda-rust-vX.Y.Z-macos-arm64.tar.gz`: system JavaScriptCore.
- `lightningpanda-rust-vX.Y.Z-macos-x64.tar.gz`: system JavaScriptCore.
- `lightningpanda-rust-vX.Y.Z-macos-arm64-fork.tar.gz`: bundled patched JavaScriptCore.
- `povio-lightning-panda-X.Y.Z.tgz`: the same client included in all binary archives.
- `lightningpanda-jsc-source-vX.Y.Z.tar.gz`: matching patched JavaScriptCore source.
- `SHA256SUMS.txt`: hashes of the five payload assets after signing and packing.

The standard builds use `LP_JSC=system`; the fork build uses the matching
`LP_JSC_DIR`. They are separate builds: source JSC selects a different module-loader
shim and timer integration, so changing only a binary's dynamic-library path does
not convert it to a system-JSC build. Build the browser and comparison worker with
`--features render -p lightningpanda -p lp-render` from the repository directory so
its Cargo linker/signing configuration applies. Keep each completed build in a
separate staging location before building another variant.

Stage both standard macOS binaries in `packages/lightning-panda/native/` as
described in [NPM_PUBLISHING.md](NPM_PUBLISHING.md), then use
`scripts/pack-test-client.sh OUT_DIR` once. Unpack that exact resulting TGZ as
`lightning-panda/` beside the browser in every platform archive. Keep `LICENSE`
and `NOTICE` in each archive. Signing or relocation must affect staged copies,
never the original build outputs or the cached JavaScriptCore library.

## Bundled JavaScriptCore

Place the copied patched library at `lib/libJavaScriptCore.dylib`. Change the
copied browser's dependency to `@loader_path/lib/libJavaScriptCore.dylib` and the
copied library's install ID to `@rpath/libJavaScriptCore.dylib`. The comparison
worker currently has no JavaScriptCore dependency. Re-sign the relocated library,
then both executables; executables retain `scripts/macos/jit.entitlements`.
Verify signatures and reject dynamic dependencies on a developer's home directory
or build cache.

The fork archive includes upstream license/notices files under
`third-party/JavaScriptCore/` and a pointer to its matching source asset. Preserve
all upstream license headers and license files in the source asset. Include the
actual sparse WebKit source tree used for the build, including files introduced
by patches that Git may still classify as untracked. Exclude `.git` and generated
build directories, not source directories that happen to contain build tooling.
Also include `REVISION`, exact patches, the original build script and provenance.

The provided `webkit/` source is **already patched**. Rebuild it directly with the
same JSC-only CMake configuration; do not apply the patches again:

```sh
cmake -G Ninja -S webkit -B build \
  -DPORT=JSCOnly -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_OSX_DEPLOYMENT_TARGET=14.0 \
  -DENABLE_STATIC_JSC=OFF -DDEVELOPER_MODE=OFF -DENABLE_TOOLS=OFF
ninja -C build -j 2 JavaScriptCore
```

Use a real Ruby executable if the build environment's version-manager shim does
not work from the build directory. The built library is under
`build/lib/JavaScriptCore.framework/Versions/`; copy its `JavaScriptCore` binary,
set its install ID as described above and sign the copy. The accompanying
`lightningpanda/scripts/jsc/build.sh` can instead fetch a **fresh** upstream
checkout and apply the recorded inputs; do not point it at the supplied
already-patched tree.

## Validate before publishing

Extract archives into a fresh directory and inspect architecture, dependencies,
code signatures and JIT entitlements. Smoke the extracted ARM builds with the
matching SDK. The fork archive must report `fork: true`; standard archives report
`fork: false`. Exercise the bundled client's automatic binary discovery as well
as explicit `LIGHTNINGPANDA_BIN`, prepared-page retirement and screenshot capture
and comparison. Confirm the fork process loads its bundled library rather than a
build-cache path. Check the source archive includes its recorded revision,
patches, introduced files and notices.

Record Intel execution only if it actually ran; cross-compilation and dependency
checks are distinct evidence. Generate checksums last, upload only completed
payloads, then verify downloaded release assets against those checksums. Keep
release notes explicit about supported capabilities and validation limits.
