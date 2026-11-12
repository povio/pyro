# Installation

Pyro is one npm package, `@povio/lightning-panda`. It contains the browser binaries, the `lightningpanda` CLI and the TypeScript client. Installing it runs no install scripts and downloads nothing else.

::: info Names in code
Pyro is the product name. The package, command and APIs keep their current names: the npm package `@povio/lightning-panda`, the `lightningpanda` command, the `Lightningpanda` class, the `Lightningpanda.*` CDP domain and the `LIGHTNINGPANDA_*` environment variables. Use those names in code and scripts.
:::

## Install the package

Add it as a development dependency:

::: code-group

```sh [bun]
bun add --dev @povio/lightning-panda
```

```sh [npm]
npm install --save-dev @povio/lightning-panda
```

:::

Release candidates are published under the `next` dist-tag:

::: code-group

```sh [bun]
bun add --dev @povio/lightning-panda@next
```

```sh [npm]
npm install --save-dev @povio/lightning-panda@next
```

:::

Stable releases go to `latest`. Publishing a stable version does not move `next`, so `next` can point at an older release candidate after a stable release. Check [Changelog and versions](/reference/versions) before you pin.

## Requirements

| What | Requirement |
| --- | --- |
| Browser on macOS | macOS 14 or newer, Apple silicon or Intel |
| Browser on Linux | x64 or arm64 with glibc 2.39 or newer (Ubuntu 24.04, Debian 13, Fedora 40 or newer) |
| Client API | Node.js 22+ or Bun. Bun loads the TypeScript sources; Node loads the compiled JavaScript and type declarations. |
| `lightningpanda test` (Playwright runner) | [Bun](https://bun.sh) on `PATH`: tests run in `bun test` worker processes |
| `lightningpanda e2e` (suite runner) | Bun on `PATH`: the runner runs on Bun, and `expect` is Bun's |

Other operating systems, including Windows, can use the client against a browser running elsewhere. See [Remote browsers](#remote-browsers).

## What is bundled

The package carries one browser per platform under `native/`:

| Directory | Platform | JavaScriptCore | Fork mode |
| --- | --- | --- | --- |
| `native/darwin-arm64/` | macOS, Apple silicon | patched build, bundled | yes |
| `native/darwin-x64/` | macOS, Intel | macOS system framework | no |
| `native/linux-x64/` | Linux x64 | patched build, bundled, with its ICU libraries in `lib/` | yes |
| `native/linux-arm64/` | Linux arm64 | patched build, bundled, with its ICU libraries in `lib/` | yes |

Every bundled browser is built with rendering enabled: layout, screenshots and PDF work out of the box. Fork mode (`serve --fork`) needs the patched JavaScriptCore, which is why the Intel macOS build does not have it. See [Fork mode](/guide/fork-mode).

The package has no `os` restriction in `package.json`, so it installs everywhere. On an unsupported platform, launching a local browser fails with a clear error; connecting to a remote one still works.

## Verify the install

Run `version` through your package manager:

::: code-group

```sh [bun]
bunx lightningpanda version
```

```sh [npm]
npx lightningpanda version
```

:::

The output names the version and what the engine can do:

```text
lightningpanda 0.0.33
engine: JavaScriptCore; capabilities: Capabilities { weak_refs: true, modules: true, import_maps: true, typed_modules: true, unhandled_rejections: true, termination: true, hidden_properties: true, proxy_targets: true, object_global_context: true, fork: true, global_constants: true }
```

`fork: true` means this binary supports [fork mode](/guide/fork-mode). On Intel Macs it reads `fork: false`.

A second check runs a script in a blank page:

```sh
bunx lightningpanda eval -e 'console.log(navigator.userAgent)'
# Mozilla/5.0 (compatible; lightningpanda/0.0.33)
```

## How the binary is chosen

`Lightningpanda.launch()`, the Playwright module and the CLI pick the browser in this order:

1. the `bin` option (`Lightningpanda.launch({ bin })`, or `use: { lightningpanda: { bin } }` in a Playwright config);
2. the `LIGHTNINGPANDA_BIN` environment variable;
3. a `lightningpanda` binary next to the package, when the package was unpacked from a GitHub release archive;
4. the bundled binary for the current platform and architecture.

The CLI and `launch()` never fall back to another `lightningpanda` on your `PATH`. Set `LIGHTNINGPANDA_BIN` to use a browser you built from source or downloaded separately:

```sh
LIGHTNINGPANDA_BIN=/opt/lightningpanda/lightningpanda bunx lightningpanda test
```

::: warning Copying binaries on macOS
The macOS binaries are signed ad hoc with the `com.apple.security.cs.allow-jit` entitlement. Without it, JavaScriptCore runs every script in its interpreter, which is much slower. Copying the file keeps the signature; re-signing it, or stripping it, removes the entitlement. Copy the whole `native/<platform>/` directory, including `lib/` when present. See [Platforms](/guide/platforms#the-jit-entitlement-on-macos) for how to re-sign.
:::

## Remote browsers

You can run the browser on one machine and the client on another. This is how Windows machines, and containers without a supported glibc, use Pyro.

On the browser host, listen on an address the client can reach:

```sh
lightningpanda serve --host 0.0.0.0 --port 9222 --advertise-host browser.internal
```

`--advertise-host` is the host name written into `/json/version`, so clients that discover the WebSocket URL over HTTP get one they can reach. The server has no authentication: expose it only on a trusted network.

On the client, connect instead of launching:

```ts
import { Lightningpanda } from '@povio/lightning-panda';

const lp = await Lightningpanda.connect('ws://browser.internal:9222/');
```

For Playwright-style tests, set `LIGHTNINGPANDA_WS=ws://browser.internal:9222/` or `use: { lightningpanda: { wsEndpoint } }`. Installing or importing the client never starts a local browser by itself.

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
      - run: bunx lightningpanda version
      - run: bunx lightningpanda e2e e2e --concurrency 8 --junit junit.xml
```

For containers, use a base image with glibc 2.39 or newer, such as `ubuntu:24.04` or `debian:trixie`. The Linux browser links `libfontconfig.so.1` and `libstdc++.so.6`. Slim images may lack fontconfig and fonts; install them if text is missing from screenshots and PDFs:

```dockerfile
FROM ubuntu:24.04
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates libfontconfig1 fonts-dejavu-core \
 && rm -rf /var/lib/apt/lists/*
```

Linux browsers are built and tested in CI only; the project publishes no Linux performance measurements yet. [CI recipes](/e2e/ci) has complete pipelines, including sharding.

## Building from source

The browser builds with Cargo from the [repository](https://github.com/povio/lightningpanda) on macOS. Source builds are not needed to use the package; they matter if you want profile-guided optimization or a patched engine. The repository README covers the build, the JIT signing contract and the optional PGO flow.
