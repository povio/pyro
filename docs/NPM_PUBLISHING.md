# Publishing @povio/pyro

One public npm package contains the testing API, `pyro` CLI launcher,
and two native browsers: `native/darwin-arm64/pyro` and
`native/darwin-x64/pyro`. The source package lives at
`packages/pyro`.
The browsers require macOS 14 or newer, or Linux with glibc 2.39+ (Ubuntu 24.04,
Debian 13, Fedora 40 or newer) on x64 or arm64; `native/linux-<arch>/` holds
the browser (patched JavaScriptCore linked in) with its ICU in `lib/`. Remote CDP connections can be used from
other operating systems; the package deliberately has no `os` restriction.

Both browsers have rendering enabled. The **arm64 browser is the fork-capable
build**: the patched JavaScriptCore is linked into it (a static archive,
`scripts/jsc/build.sh`), so fork mode (`--fork`) and the patched engine's fast
paths work out of the box with no library beside it. The Intel browser uses
**system JavaScriptCore** (the patched library is built for arm64 only), so it
has no fork mode. `npm pack` refuses a `native/darwin-arm64/lib` left by a
build that still shipped the library. No install-time scripts or downloads run.

## One-time npm setup

An npm account with publish rights to the `@povio` scope is required. The workflow
is `.github/workflows/publish-npm.yml`; it publishes only after testing the exact
packed tarball on both macOS architectures with Node 22 and Bun, and verifying
that the client imports on Linux.

Publish the first version locally using your npm account with 2FA. With both
native binaries staged as described below, run:

```sh
cd packages/pyro
bun install --frozen-lockfile
npm login
npm pack
npm publish ./povio-pyro-0.0.17.tgz --access public --tag latest
```

Use the tarball filename for the actual package version. `npm pack` runs the
build and packaging checks automatically. For an RC version, use `--tag next`.
This path needs no GitHub `NPM_TOKEN` secret. npm account permissions and an
unused package version are still required.

Alternatively, bootstrapping through GitHub Actions is supported by adding a
repository secret named `NPM_TOKEN` with a granular npm token permitted to
create/publish this package and bypass 2FA. It is exposed only to the publish
step; remove it after switching to trusted publishing.

After the first publication, configure **Trusted publishing** in the npm
package settings:

- Provider: GitHub Actions
- Organization/user: `povio`
- Repository: `pyro`
- Workflow filename: `publish-npm.yml`
- Environment: leave blank (the workflow does not use a GitHub Environment)

Leave `NPM_TOKEN` unset (or remove it if used). The publish job has `id-token: write` and uses Node 24
and npm 11 for OIDC publishing. npm automatically adds provenance for trusted
publishing from a public repository. See
[npm trusted publishing](https://docs.npmjs.com/trusted-publishers/).

Commit and push the workflow before the next release tag. The first automated
release must use a new version; tagging the version already published locally
would try to publish it again and fail.
Configure the trusted publisher when ready for that release: npm requires its
first successful publication within two days of configuration, or the
configuration expires and must be recreated.

## Release tags

| Git tag | npm version | npm dist-tag |
| --- | --- | --- |
| `v0.0.18` | `0.0.18` | `latest` |
| `v0.0.18-rc.1` | `0.0.18-rc.1` | `next` |
| `v0.0.18-rc` | `0.0.18-rc` | `next` |

Tags must include `v`. Other prerelease labels, build metadata and malformed tags
fail validation before any browser build or publication. Publishing a stable
version does not move `next`; that tag continues pointing at the last RC.

The Git tag is the source of truth. **No version edits are needed** in
`package.json`, `Cargo.toml`, or `Cargo.lock` before a GitHub release.
Commit and push the code you want to release, then create and push the tag:

```sh
git tag v0.0.18-rc.1
git push origin v0.0.18-rc.1
```

Pushing the tag starts the publishing workflow. CI writes the version (without
the leading `v`, including any RC suffix) into the npm manifest and Cargo
workspace manifest, then runs `cargo update --workspace` to refresh workspace
versions in `Cargo.lock` while preserving locked external dependencies. Both
native builds and npm packaging use the same generated manifests. Native builds
still run with `--locked`, and the browser's reported version is checked against
the tag. These version edits stay in CI; they are not committed back to the branch.

Use a new version for each publication: npm versions cannot be reused.
If a job fails before publication, re-run the failed workflow jobs after fixing
external configuration, or create a new version/tag for source changes.

The publish step tries `npm publish` up to four times and succeeds once the
version is on the registry, so re-running it after a partial failure is safe.
A transient Sigstore failure (npm `TLOG_CREATE_ENTRY_ERROR`, 409 "an equivalent
entry already exists in the transparency log": a provenance record that timed
out on the client after Rekor stored it) happens before the upload and is
retried with a new signature. v0.0.45 failed that way with no retry: its GitHub
release has every asset, but npm never got 0.0.45.

## Local package checks

Local `npm publish` uses the version in the local manifest; it does not infer a
Git tag. To prepare a different version locally without editing manifests by
hand, run from the repository root **before rebuilding the native binaries**:

```sh
node scripts/npm/release.mjs v0.0.18-rc.1 --write
cargo update --workspace
```

Without `--write`, the release script only validates the tag and prints its
version and npm dist-tag. It does not change any files.

```sh
cd packages/pyro
bun install --frozen-lockfile
bun run build
bun run test:package
```

To pack a complete release locally, build on both native architectures with
`PYRO_JSC=system cargo build --locked --release -p pyro --features render`
from the repository root. Copy each output to its corresponding
`packages/pyro/native/darwin-{arm64,x64}/pyro` path.
Preserve the binaries' ad hoc signatures and JIT entitlements. Then run:

```sh
cd packages/pyro
npm pack
```

`prepack` builds JavaScript and declarations, verifies both Mach-O architectures,
sets executable permissions, and copies the root `LICENSE` and `NOTICE`. It
refuses incomplete packages. The generated native files, `dist/`, copied notices
and tarballs are ignored by git. The tarball is named
`povio-pyro-<version>.tgz`.

`scripts/pack-test-client.sh OUT_DIR [ARCHIVE_DIR]` uses the same prepack checks
and can additionally unpack the package into a legacy archive's
`pyro-client/` directory.

The workflow's `npm-package` artifact is the exact tarball installed by smoke
tests and subsequently published. To test it manually, install it into a clean
temporary project with `npm install --ignore-scripts /absolute/path/package.tgz`,
run `./node_modules/.bin/pyro version`, and run
`node /absolute/path/to/repo/scripts/npm/smoke.mjs` from that project.
