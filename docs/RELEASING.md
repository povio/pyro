# Release packaging

## In CI (the normal way)

Push a tag `vX.Y.Z` (or `vX.Y.Z-rc.N`) on the commit to release. The
`publish-npm.yml` workflow then does everything below without local work:

- stamps the versions from the tag;
- builds the standard browsers and comparison workers for ARM64 and Intel;
- builds the fork variant against the patched JavaScriptCore (`jsc.yml`, which
  restores it from the Actions cache, or builds it on a miss);
- builds Linux x64 and arm64 against the Linux build of the same patched
  JavaScriptCore (`linux-build.yml`), bundles it with its ICU, and runs the
  client suite against each;
- exports the patched JavaScriptCore source (`scripts/release/jsc-source.sh`);
- assembles, signs and checks the three archives (`scripts/release/assemble.sh`);
- tests the client against the extracted ARM64 browsers;
- publishes the npm package and creates the GitHub release with all six assets
  and generated notes, then downloads them and verifies the checksums.

A release candidate tag makes a GitHub prerelease and publishes to npm `next`.
Linux release executables have debug information removed from staged copies
before bundle tests; build outputs and runtime symbols are retained. Published
Linux binaries therefore do not include source-line debug information. The
combined npm tarball must fit the workflow's 150 MiB compressed-size budget;
this guards package growth but does not guarantee registry acceptance.

A cold JavaScriptCore build takes about 7 minutes on a hosted runner (measured on
the first run). `jsc.yml` rebuilds on `main` whenever the patches or
`scripts/jsc/build.sh` change, so a tag normally finds the build cached.

The npm package's arm64 browser is the fork build, with the patched
JavaScriptCore linked into it; its Intel browser is the standard build. Every release archive's client therefore carries the fork browser, but
launches the browser of its own archive first.
Intel binaries are inspected in CI but never executed (no Rosetta on the
runners).

The rest of this document is what those scripts do, and the manual procedure
for a release built outside CI.

## Contents

Release assets contain a render-enabled `pyro`, the matching
`pyro-snapshot-compare`, and one identical `pyro` payload. For npm
releases, the [tag-triggered workflow](NPM_PUBLISHING.md#release-tags) sets all
versions automatically. For manual archive builds, run
`node scripts/npm/release.mjs vX.Y.Z --write` and `cargo update --workspace`
before building. Record the source commit,
input hashes and validation results; packaging alone is not functional testing.

Publish these six assets for a release version `X.Y.Z`:

- `pyro-rust-vX.Y.Z-macos-arm64.tar.gz`: system JavaScriptCore.
- `pyro-rust-vX.Y.Z-macos-x64.tar.gz`: system JavaScriptCore.
- `pyro-rust-vX.Y.Z-macos-arm64-fork.tar.gz`: patched JavaScriptCore, linked statically.
- `povio-pyro-X.Y.Z.tgz`: the same client included in all binary archives.
- `pyro-jsc-source-vX.Y.Z.tar.gz`: matching patched JavaScriptCore source.
- `SHA256SUMS.txt`: hashes of the five payload assets after signing and packing.

The standard builds use `PYRO_JSC=system`; the fork build uses the matching
`PYRO_JSC_DIR`. They are separate builds: source JSC selects a different module-loader
shim and timer integration, and is linked into the binary. Build the browser and comparison worker with
`--features render -p pyro -p pyro-render` from the repository directory so
its Cargo linker/signing configuration applies. Keep each completed build in a
separate staging location before building another variant.

Stage both standard macOS binaries in `packages/pyro/native/` as
described in [NPM_PUBLISHING.md](NPM_PUBLISHING.md), then use
`scripts/pack-test-client.sh OUT_DIR` once. Unpack that exact resulting TGZ as
`pyro-client/` beside the browser in every platform archive. Keep `LICENSE`
and `NOTICE` in each archive. Signing must affect staged copies, never the
original build outputs.

## Patched JavaScriptCore

The fork browser holds the patched JavaScriptCore: `scripts/jsc/build.sh` builds
one static archive (`lib/libJavaScriptCore.a`: JavaScriptCore, WTF, bmalloc) and
the build links it in, so the archive ships no library beside the browser (macOS;
Linux ships the ICU libraries the browser loads in `lib/`,
`scripts/release/bundle-icu-linux.sh`). The executables keep the signature and
`scripts/macos/jit.entitlements` the build's linker wrapper gave them. Verify
signatures and reject dynamic dependencies outside `/System` and `/usr/lib`.

JavaScriptCore is LGPL: a user who rebuilds it from the source asset relinks the
browser from this repository's source (AGPL, public) with `PYRO_JSC_DIR` pointing at
their build.

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
  -DENABLE_STATIC_JSC=ON -DDEVELOPER_MODE=OFF -DENABLE_TOOLS=OFF
pyro/scripts/jsc/archive.sh build out/lib/libJavaScriptCore.a
```

Use a real Ruby executable if the build environment's version-manager shim does
not work from the build directory. `archive.sh` builds the static libraries and
merges them, with the JIT subtarget's objects, into the one archive the browser
links (`PYRO_JSC_DIR=out`, with `inputs/` copied from a released build or from
`crates/pyro-jsc-sys/webkit`). The accompanying
`pyro/scripts/jsc/build.sh` can instead fetch a **fresh** upstream
checkout and apply the recorded inputs; do not point it at the supplied
already-patched tree.

## Validate before publishing

Extract archives into a fresh directory and inspect architecture, dependencies,
code signatures and JIT entitlements. Smoke the extracted ARM builds with the
matching SDK. The fork archive must report `fork: true`; standard archives report
`fork: false`. Exercise the bundled client's automatic binary discovery as well
as explicit `PYRO_BIN`, prepared-page retirement and screenshot capture
and comparison. Confirm the fork process loads its bundled library rather than a
build-cache path. Check the source archive includes its recorded revision,
patches, introduced files and notices.

Record Intel execution only if it actually ran; cross-compilation and dependency
checks are distinct evidence. Generate checksums last, upload only completed
payloads, then verify downloaded release assets against those checksums. Keep
release notes explicit about supported capabilities and validation limits.
