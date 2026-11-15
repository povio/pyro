# Platforms and limits

Pyro's browser runs on macOS and Linux. The client runs anywhere Node 22+ or Bun runs, and can drive a browser on another machine.

## Supported platforms

| Platform | Browser | Fork mode | JavaScriptCore | Tested in the release workflow |
| --- | --- | --- | --- | --- |
| macOS 14+, Apple silicon | bundled | yes | patched, linked into the binary | Apple silicon Mac (self-hosted) |
| macOS 14+, Intel | bundled | no | macOS system framework | `macos-15-intel` runner |
| Linux x64, glibc 2.39+ | bundled | yes | patched, linked into the binary; ICU in `lib/` | `ubuntu-24.04` runner |
| Linux arm64, glibc 2.39+ | bundled | yes | patched, linked into the binary; ICU in `lib/` | `ubuntu-24.04-arm` runner |
| Windows | none | | | client only, against a remote browser |
| Linux with older glibc, Alpine (musl) | none | | | client only, against a remote browser |

glibc 2.39 or newer means Ubuntu 24.04, Debian 13, Fedora 40 or newer. Check yours with `ldd --version`.

The client (`@povio/pyro`) needs Node.js 22+ or Bun. The two test runners, `pyro run` and `pyro e2e`, need Bun on `PATH`.

## Intel Macs

The Intel build uses the JavaScriptCore that ships with macOS instead of the patched one. Everything works except what needs the patch:

- **No fork mode.** `serve --fork` exits with an error; `pyro e2e` falls back to thread mode, where each page boots its profile.
- Features that the system framework does not offer fall back or report an error: import maps are ignored with a console warning, and precise JavaScript coverage (`page.coverage`) is not available.

Use [profiles](/guide/profiles) with several connections to hide boot time on Intel machines.

## The JIT entitlement on macOS

macOS's JavaScriptCore only compiles JavaScript to machine code in a process that holds the `com.apple.security.cs.allow-jit` entitlement. Without it, every script runs in the interpreter: correct, but much slower.

The bundled binaries are signed ad hoc with that entitlement and nothing else. You keep it as long as you copy the binaries as they are. You lose it if you re-sign or strip them. To check a binary:

```sh
codesign -d --entitlements - path/to/pyro
# should list com.apple.security.cs.allow-jit = true
```

To restore it, sign with an entitlements file that contains that key:

```sh
cat > jit.entitlements <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>com.apple.security.cs.allow-jit</key>
  <true/>
</dict>
</plist>
EOF
codesign -f -s - --entitlements jit.entitlements path/to/pyro
```

When you copy a browser out of the package, copy its whole `native/<platform>/` directory: the Linux builds load ICU libraries from the `lib/` directory next to the binary.

## Windows

There is no Windows browser. Run the browser on a macOS or Linux machine (or a Linux VM, or WSL 2 with a glibc 2.39+ distribution) and connect to it from Windows:

```sh
# on the browser host
pyro serve --host 0.0.0.0 --port 9222 --advertise-host pyro-host
```

```ts
// on Windows
const pyro = await Pyro.connect('ws://pyro-host:9222/');
```

For Playwright-style tests on Windows, set `PYRO_WS=ws://pyro-host:9222/`. Note that `pyro run` and `pyro e2e` still need Bun on the Windows machine, and that paths in server options (`--static`, file uploads by path) refer to the browser host. The server has no authentication; keep it on a trusted network.

## Docker and Linux notes

- Use a base image with glibc 2.39 or newer: `ubuntu:24.04`, `debian:trixie`, or newer. Alpine and other musl-based images do not work.
- The Linux browser links `libfontconfig.so.1` and `libstdc++.so.6`, and on x64 also `libatomic.so.1` (from JavaScriptCore). Its JavaScriptCore is linked into the binary and its ICU libraries ship in `lib/`. Slim images need fontconfig and at least one font package for text in screenshots and PDFs.
- To reach the browser from outside the container, start it with `--host 0.0.0.0` and publish the port. Pass `--advertise-host` with a name clients can resolve if they read `/json/version`.
- Linux browsers are built and tested in CI only. The project has not yet published Linux performance numbers.

```dockerfile
FROM ubuntu:24.04
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl unzip libatomic1 libfontconfig1 fonts-dejavu-core \
 && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL https://bun.sh/install | bash
ENV PATH=/root/.bun/bin:$PATH
WORKDIR /app
COPY package.json bun.lock ./
RUN bun install --frozen-lockfile
COPY . .
CMD ["bunx", "pyro", "serve", "--host", "0.0.0.0", "--port", "9222"]
```

## Limits worth knowing

| Limit | Default | Change with |
| --- | --- | --- |
| Concurrent CDP connections | 16; more wait in the kernel's backlog | `--cdp-max-connections` |
| CDP WebSocket message size | 64 MiB | `--cdp-max-message-size` |
| CDP HTTP message size | 1 MiB | `--cdp-max-http-message-size` |
| `Pyro.run` step timeout | 5 s per step | `page.run(steps, { timeout })` or per step |
| `Pyro.run` maximum timeout | 600 s | |
| Steps per `Pyro.run` | 10,000 | |
| Settle wait after an action | 1 s, then the run carries on | `{ settle: false }` to skip |
| Pages per profile pool | 1 to 8 | `pool` |
| Fork spares per template | 2 | `PYRO_FORK_SPARES` |

Web platform coverage is partial: WebGL, WebGPU, WebRTC and `SharedWorker` are absent, among others. See [Web platform support](/reference/web-platform).
