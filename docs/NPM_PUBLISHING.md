# Publishing @povio/lightning-panda

One public npm package contains the testing API, `lightningpanda` CLI launcher,
and two native browsers: `native/darwin-arm64/lightningpanda` and
`native/darwin-x64/lightningpanda`. The source package lives at
`packages/lightning-panda`.
The browsers require macOS 14 or newer. Remote CDP connections can be used from
other operating systems; the package deliberately has no `os` restriction.

The npm builds use **system JavaScriptCore**, with rendering enabled. Fork mode
is not included; use a separately built patched browser via `LIGHTNINGPANDA_BIN`
or `Lightningpanda.launch({ bin })`. No install-time scripts or downloads run.

## One-time npm setup

An npm account with publish rights to the `@povio` scope is required. The workflow
is `.github/workflows/publish-npm.yml`; it publishes only after testing the exact
packed tarball on both macOS architectures with Node 22 and Bun, and verifying
that the client imports on Linux.

Publish the first version locally using your npm account with 2FA. With both
native binaries staged as described below, run:

```sh
cd packages/lightning-panda
bun install --frozen-lockfile
npm login
npm pack
npm publish ./povio-lightning-panda-0.0.17.tgz --access public --tag latest
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
- Repository: `lightningpanda`
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

1. Set the **same complete version**, including any `-rc.N` suffix, in the
   root `Cargo.toml` (`workspace.package.version`) and
   `packages/lightning-panda/package.json`.
2. Run `cargo check -p lightningpanda` from the repository root to update
   workspace package versions in `Cargo.lock`; commit the lockfile with both
   manifests. The workflow builds with `--locked`.
3. Validate locally, for example:
   `node scripts/npm/release.mjs v0.0.18-rc.1`.
4. Commit and push the release changes, then create and push that tag:

   ```sh
   git tag v0.0.18-rc.1
   git push origin v0.0.18-rc.1
   ```

Pushing the tag starts the publishing workflow. The tag, npm manifest and Cargo
manifest must match exactly; the workflow does not silently rewrite versions.
Use a new version for each publication: npm versions cannot be reused.
If a job fails before publication, re-run the failed workflow jobs after fixing
external configuration, or create a new version/tag for source changes.

## Local package checks

```sh
cd packages/lightning-panda
bun install --frozen-lockfile
bun run build
bun run test:package
```

To pack a complete release locally, build on both native architectures with
`LP_JSC=system cargo build --locked --release -p lightningpanda --features render`
from the repository root. Copy each output to its corresponding
`packages/lightning-panda/native/darwin-{arm64,x64}/lightningpanda` path.
Preserve the binaries' ad hoc signatures and JIT entitlements. Then run:

```sh
cd packages/lightning-panda
npm pack
```

`prepack` builds JavaScript and declarations, verifies both Mach-O architectures,
sets executable permissions, and copies the root `LICENSE` and `NOTICE`. It
refuses incomplete packages. The generated native files, `dist/`, copied notices
and tarballs are ignored by git. The tarball is named
`povio-lightning-panda-<version>.tgz`.

`scripts/pack-test-client.sh OUT_DIR [ARCHIVE_DIR]` uses the same prepack checks
and can additionally unpack the package into a legacy archive's
`lightning-panda/` directory.

The workflow's `npm-package` artifact is the exact tarball installed by smoke
tests and subsequently published. To test it manually, install it into a clean
temporary project with `npm install --ignore-scripts /absolute/path/package.tgz`,
run `./node_modules/.bin/lightningpanda version`, and run
`node /absolute/path/to/repo/scripts/npm/smoke.mjs` from that project.
