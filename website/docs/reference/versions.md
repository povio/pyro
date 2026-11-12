# Changelog and versions

Pyro ships as one npm package, `@povio/lightning-panda`, that holds the SDK, the `lightningpanda` CLI and the native browsers. Every release is cut from a Git tag; this page lists the channels, the platforms, how to pin a version and what changed in recent releases.

## Package

| | |
| --- | --- |
| npm name | `@povio/lightning-panda` |
| License | AGPL-3.0-only |
| Binary | `lightningpanda` (`node_modules/.bin/lightningpanda`) |
| Entry points | `@povio/lightning-panda` (SDK), `@povio/lightning-panda/e2e` (suite runner API), `@povio/lightning-panda/playwright` and `@povio/lightning-panda/test` (Playwright drop-in) |
| Install scripts | None. The browsers are inside the tarball; nothing is downloaded at install time. |

Check what you have installed:

```sh
bunx lightningpanda version
# lightningpanda 0.0.33
# engine: JavaScriptCore; capabilities: Capabilities { ..., fork: true, ... }
```

`fork: true` means the browser was built against the patched JavaScriptCore and supports [fork mode](/reference/serve#fork).

## Release channels {#channels}

| npm dist-tag | Git tag | What it holds |
| --- | --- | --- |
| `latest` | `vX.Y.Z` | Stable releases. `bun add @povio/lightning-panda` installs this. |
| `next` | `vX.Y.Z-rc.N` or `vX.Y.Z-rc` | Release candidates. Publishing a stable version does not move `next`; it keeps pointing at the last RC. |

As of 2026-11-12, `latest` is `0.0.33` and `next` is `0.0.17-rc.1` (an old candidate, older than `latest`). Use `next` only when a release candidate is announced.

```sh
bun add @povio/lightning-panda          # latest
bun add @povio/lightning-panda@next     # newest release candidate
npm view @povio/lightning-panda dist-tags
```

## Platforms and runtimes {#platforms}

| Platform | Browser in the package | Fork mode |
| --- | --- | --- |
| macOS 14+ arm64 (Apple silicon) | `native/darwin-arm64`, patched JavaScriptCore | Yes |
| macOS 14+ x64 (Intel) | `native/darwin-x64`, system JavaScriptCore | No |
| Linux x64, glibc 2.39+ (Ubuntu 24.04, Debian 13, Fedora 40 or newer) | `native/linux-x64`, patched JavaScriptCore, ICU bundled in `lib/` | Yes |
| Linux arm64, glibc 2.39+ | `native/linux-arm64`, patched JavaScriptCore, ICU bundled in `lib/` | Yes |

Linux builds ship since v0.0.27 (on npm since 0.0.28). Release Linux binaries have no source-line debug information. The Intel macOS binary is checked in CI but not executed there.

On any other OS, or to use a browser you built yourself, use [`Lightningpanda.connect()`](/reference/sdk) against a remote `lightningpanda serve`, or set [`LIGHTNINGPANDA_BIN`](/reference/env). The package has no `os` restriction, so it installs everywhere.

| Runtime | Supported |
| --- | --- |
| Node.js | 22 or newer (`engines.node: ">=22"`) |
| Bun | Supported. Release CI tests the packed tarball with Bun 1.4.0; no minimum is declared. |

## Pinning a version {#pinning}

Versions are immutable on npm. Pin the exact version so a CI run and a laptop run the same browser:

```sh
bun add --exact @povio/lightning-panda@0.0.33
```

```json
{
  "devDependencies": {
    "@povio/lightning-panda": "0.0.33"
  }
}
```

The browser binary is part of the package, so pinning the package pins the browser. `lightningpanda version` prints the version the binary was built as; CI checks that it matches the tag.

Versions are `0.0.x`: any release may change behaviour. Read the notes below before upgrading.

## Release notes {#release-notes}

Each tag `vX.Y.Z` creates a GitHub release named "Rust Lightpanda vX.Y.Z" in `povio/lightningpanda`, with generated notes (the merged pull requests) and these assets:

| Asset | Contents |
| --- | --- |
| `lightningpanda-rust-vX.Y.Z-macos-arm64.tar.gz` | Browser on system JavaScriptCore, `lp-snapshot-compare`, the client |
| `lightningpanda-rust-vX.Y.Z-macos-x64.tar.gz` | Same, Intel |
| `lightningpanda-rust-vX.Y.Z-macos-arm64-fork.tar.gz` | Browser with the patched JavaScriptCore (fork mode) |
| `lightningpanda-rust-vX.Y.Z-linux-x64.tar.gz`, `...-linux-arm64.tar.gz` | Linux browsers, ICU in `lib/` |
| `povio-lightning-panda-X.Y.Z.tgz` | The npm package |
| `lightningpanda-jsc-source-vX.Y.Z.tar.gz` | The patched JavaScriptCore source (LGPL) |
| `SHA256SUMS.txt` | Checksums of the assets |

The repository is private at the time of writing, so the GitHub releases need repository access. The npm package is public.

A new version can take a while to appear on npm after its release: npm has listed versions up to about an hour after publication. If `bun add @povio/lightning-panda@X.Y.Z` cannot find a fresh version, wait and retry.

## Unreleased (main) {#unreleased}

On `main` after v0.0.33:

- **Faster connection start and fork mode** (#303): no JavaScript realm is created before a connection's first command; the fork template is forked from the server process.
- **Engine reuse across connections** (#304): connections reuse the engine; the patched JavaScriptCore is linked statically (the arm64 macOS package no longer carries a `lib/` directory beside the browser); memory is returned after pages close.
- **Vendored engines updated** (#302): Blitz, Taffy and Stylo 0.24.0.

## Version history {#history}

### 0.0.33 (2026-11-11)

- Fork mode: the root template compiles the app's ES modules once; forked pages load them without parsing (#301).
- Fork mode: the fork holds JavaScriptCore's timer thread and each forked child arms its own watchdog timer (#300).

### 0.0.32 (2026-11-11)

- New `serve --hyphen-digit-breaks` (`LIGHTNINGPANDA_HYPHEN_DIGIT_BREAKS`): Chrome's line break between a hyphen and a digit (#299).

### 0.0.31 (2026-11-10)

- PDF: variable fonts print at the weight and optical size they were shaped at (#294).
- PDF: glyphs substituted by OpenType features extract as their source text on page-box PDFs (#292).
- Fonts: `@font-face` `unicode-range` decides which face draws a character (#293).
- XML documents stop at each script (#296).
- WPT coverage gate on every pull request (#276, #295).

### 0.0.30 (2026-11-10)

- New `lightningpanda e2e` suite runner: one process, one browser per suite (#289). See [the runner guide](/e2e/runner).
- Shared profiles across clients of one `serve --fork`, `serve` lifetime fixes (#289).
- Fork mode: less CPU per forked page (fast exit, 32 MB JIT pool, JavaScriptCore heap growth) (#291).

### 0.0.29 (2026-11-10)

- Doubles cross CDP unchanged (float round-trip) (#290).

### 0.0.28 (2026-11-10)

- First npm release with the Linux browsers (smaller Linux payload).
- Fixed viewport invalidation on unstyled nodes.

### 0.0.27 (2026-11-10)

- Linux x64 and arm64 builds, in CI and in the GitHub release (#288). Not published to npm.

### 0.0.26 (2026-11-09)

- Module fetches are shared; service worker request ownership is preserved.

### 0.0.25 (2026-11-09)

- Fork IPC survives interrupted system calls.

### 0.0.24 (2026-11-09)

- A click's stability frames no longer wait an extra turn each (#287).

### 0.0.23 (2026-11-09)

- Bursts of cached resources no longer close CDP connections.

### 0.0.22 (2026-11-08)

- Playwright suites run on Pyro by swapping the import (#278). See [Coming from Playwright](/playwright/overview).
- Web-platform gaps found by the WPT survey filled.
- PDF pagination at Chrome's speed (#282); incremental tables and upstream Blitz/Taffy/Stylo (#279); whole-program performance passes (#283-#286).

### Earlier releases

| Version | Date | Notes |
| --- | --- | --- |
| 0.0.21 | 2026-11-06 | DOM, binding, CDP and style data maintained at mutation time (#274); deferred screenshot state preserved, passwords masked (#272) |
| 0.0.20 | 2026-11-05 | Release CI fix |
| 0.0.19 | 2026-11-05 | The npm package ships the fork-capable arm64 browser (#271); GitHub release assets built from the tag (#270). npm only, no GitHub release. |
| 0.0.18 | 2026-11-05 | DOM calls through the JITs, lazy startup (#268); WPT parity on the bench subset (#269); table layout fixes |
| 0.0.17 | 2026-11-02 | First npm release (published 2026-11-04) |
| 0.0.17-rc.1 | 2026-11-04 | Tag-based npm publishing; the current `next` |
| 0.0.1 to 0.0.16 | 2026-10-20 to 2026-11-02 | GitHub releases only |
