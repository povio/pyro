# Installation

Pyro is one npm package, `@povio/pyro`. It contains the browser binaries, the `pyro` CLI and the TypeScript client. Installing it runs no install scripts and downloads nothing else.

::: info Formerly Lightningpanda
Pyro was called Lightningpanda up to version 0.0.33, published as `@povio/lightning-panda` with the `lightningpanda` command. The rename moved every name, with no aliases: class `Pyro`, `PYRO_*` environment variables, the `Pyro.*` CDP domain, `@povio/pyro/wright` and `pyro run` for Playwright tests. These docs use the new names. See [Versions](/reference/versions#renamed-from-lightningpanda) for the full mapping.
:::

## Before the first `@povio/pyro` release {#try-today}

`@povio/pyro` is not on npm yet. The next release is the first published under that name, and the install commands below work from then on. To try Pyro today, either:

- install `@povio/lightning-panda` (0.0.33, `bun add --dev @povio/lightning-panda`) and use the old names from the table in [Versions](/reference/versions#renamed-from-lightningpanda). It lacks the changes made on `main` since 0.0.33; or
- build the browser from a checkout of the [repository](https://github.com/povio/pyro) (see [Building from source](#building-from-source)), build the client there, install it from the checkout, and point it at your browser. A checkout has no bundled browsers in `native/`, so `PYRO_BIN` is required:

  ```sh
  (cd /path/to/pyro/packages/pyro && bun install && bun run build)
  bun add --dev /path/to/pyro/packages/pyro
  PYRO_BIN=/path/to/pyro/target/release/pyro bunx pyro version
  ```

## Install the package

Add it as a development dependency:

::: code-group

```sh [bun]
bun add --dev @povio/pyro
```

```sh [npm]
npm install --save-dev @povio/pyro
```

:::

Release candidates are published under the `next` dist-tag:

::: code-group

```sh [bun]
bun add --dev @povio/pyro@next
```

```sh [npm]
npm install --save-dev @povio/pyro@next
```

:::

Stable releases go to `latest`. Publishing a stable version does not move `next`, so `next` can point at an older release candidate after a stable release. Check [Changelog and versions](/reference/versions) before you pin.

## Requirements

| What | Requirement |
| --- | --- |
| Browser on macOS | macOS 14 or newer, Apple silicon or Intel |
| Browser on Linux | x64 or arm64 with glibc 2.39 or newer (Ubuntu 24.04, Debian 13, Fedora 40 or newer) |
| Client API | Node.js 22+ or Bun. Bun loads the TypeScript sources; Node loads the compiled JavaScript and type declarations. |
| `pyro run` (Playwright runner) | [Bun](https://bun.sh) on `PATH`: tests run in `bun test` worker processes |
| `pyro e2e` (suite runner) | Bun on `PATH`: the runner runs on Bun, and `expect` is Bun's |

Other operating systems, including Windows, can use the client against a browser running elsewhere. See [Remote browsers](#remote-browsers).

## What is bundled

The package carries one browser per platform under `native/`:

| Directory | Platform | JavaScriptCore | Fork mode |
| --- | --- | --- | --- |
| `native/darwin-arm64/` | macOS, Apple silicon | patched build, linked into the binary | yes |
| `native/darwin-x64/` | macOS, Intel | macOS system framework | no |
| `native/linux-x64/` | Linux x64 | patched build, linked into the binary; its ICU libraries in `lib/` | yes |
| `native/linux-arm64/` | Linux arm64 | patched build, linked into the binary; its ICU libraries in `lib/` | yes |

Every bundled browser is built with rendering enabled: layout, screenshots and PDF work out of the box. Fork mode (`serve --fork`) needs the patched JavaScriptCore, which is why the Intel macOS build does not have it. See [Fork mode](/guide/fork-mode).

The package has no `os` restriction in `package.json`, so it installs everywhere. On an unsupported platform, launching a local browser fails with a clear error; connecting to a remote one still works.

## Verify the install

Run `version` through your package manager:

::: code-group

```sh [bun]
bunx pyro version
```

```sh [npm]
npx pyro version
```

:::

The output names the version and what the engine can do:

```text
pyro X.Y.Z
engine: JavaScriptCore; capabilities: Capabilities { weak_refs: true, modules: true, import_maps: true, typed_modules: true, unhandled_rejections: true, termination: true, hidden_properties: true, proxy_targets: true, object_global_context: true, fork: true, global_constants: true }
```

`fork: true` means this binary supports [fork mode](/guide/fork-mode). On Intel Macs it reads `fork: false`.

A second check runs a script in a blank page:

```sh
bunx pyro eval -e 'console.log(navigator.userAgent)'
# Mozilla/5.0 (compatible; pyro/X.Y.Z)
```

## How the binary is chosen

`Pyro.launch()`, the Playwright module and the CLI pick the browser in this order:

1. the `bin` option (`Pyro.launch({ bin })`, or `use: { pyro: { bin } }` in a Playwright config);
2. the `PYRO_BIN` environment variable;
3. a `pyro` binary next to the package folder, as in a GitHub release archive, where the client folder `pyro-client/` sits beside the browser;
4. the bundled binary for the current platform and architecture.

The CLI and `launch()` never fall back to another `pyro` on your `PATH`. Set `PYRO_BIN` to use a browser you built from source or downloaded separately:

```sh
PYRO_BIN=/opt/pyro/pyro bunx pyro run
```

::: warning Copying binaries on macOS
The macOS binaries are signed ad hoc with the `com.apple.security.cs.allow-jit` entitlement. Without it, JavaScriptCore runs every script in its interpreter, which is much slower. Copying the file keeps the signature; re-signing it, or stripping it, removes the entitlement. Copy the whole `native/<platform>/` directory, including `lib/` on Linux. See [Platforms](/guide/platforms#the-jit-entitlement-on-macos) for how to re-sign.
:::

## Remote browsers

You can run the browser on one machine and the client on another. This is how Windows machines, and containers without a supported glibc, use Pyro.

On the browser host, listen on an address the client can reach:

```sh
pyro serve --host 0.0.0.0 --port 9222 --advertise-host browser.internal
```

`--advertise-host` is the host name written into `/json/version`, so clients that discover the WebSocket URL over HTTP get one they can reach. The server has no authentication: expose it only on a trusted network.

On the client, connect instead of launching:

```ts
import { Pyro } from '@povio/pyro';

const pyro = await Pyro.connect('ws://browser.internal:9222/');
```

For Playwright-style tests, set `PYRO_WS=ws://browser.internal:9222/` or `use: { pyro: { wsEndpoint } }`. Installing or importing the client never starts a local browser by itself.

Paths given to the server, such as `--static` directories or file uploads given as paths, refer to the browser host's file system.

## CI

The package's own release workflow tests the packed tarball on GitHub-hosted `macos-15`, `macos-15-intel`, `ubuntu-24.04` and `ubuntu-24.04-arm` runners, with Node 22 and Bun. Those images work without extra setup. On GitHub Actions:

```yaml
jobs:
  e2e:
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v4
      - uses: oven-sh/setup-bun@v2
      - run: bun install --frozen-lockfile
      - run: bunx pyro version
      - run: bunx pyro e2e e2e --concurrency 8 --junit junit.xml
```

For containers, use a base image with glibc 2.39 or newer, such as `ubuntu:24.04` or `debian:trixie`. The Linux browser links `libfontconfig.so.1` and `libstdc++.so.6`, and on x64 also `libatomic.so.1` (from JavaScriptCore). Slim images may lack fontconfig and fonts; install them if text is missing from screenshots and PDFs:

```dockerfile
FROM ubuntu:24.04
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates libatomic1 libfontconfig1 fonts-dejavu-core \
 && rm -rf /var/lib/apt/lists/*
```

Linux browsers are built and tested in CI only; the project publishes no Linux performance measurements yet. [CI recipes](/e2e/ci) has complete pipelines, including sharding.

## Building from source

The browser builds with Cargo from the [repository](https://github.com/povio/pyro) on macOS. Source builds are not needed to use the package; they matter if you want profile-guided optimization or a patched engine. The repository README covers the build, the JIT signing contract and the optional PGO flow.
