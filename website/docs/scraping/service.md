# Running as a service

For scraping around the clock, run `pyro serve` as a long-lived service and connect your jobs to it, instead of launching a browser per script. This page covers the flags that matter for a service, a systemd unit and a Docker image for Ubuntu 24.04, health checks, logging, and how to keep the CDP port safe.

## The server

```sh
pyro serve --host 127.0.0.1 --port 9222 --cdp-max-connections 32
```

| Flag | Default | Notes |
| --- | --- | --- |
| `--host <HOST>` | `127.0.0.1` | Address to listen on. `0.0.0.0` listens on every interface; see [Securing it](#securing-it) first. |
| `--port <INT>` | `9222` | |
| `--advertise-host <HOST>` | `--host` | The host written into `webSocketDebuggerUrl` in `/json/version`. Set it when clients reach the server at another address than it binds, for example `--host 0.0.0.0 --advertise-host 10.0.0.5`. Use an IP address: a client that connects by host name gets `403` (see below). |
| `--cdp-max-connections <INT>` | `16` | Connections served at once. More wait in the kernel's backlog until one closes. In fork mode, one per open page. |
| `--cdp-max-message-size <INT>` | `67108864` (64 MiB) | Largest WebSocket message. |
| `--cdp-max-http-message-size <INT>` | `1048576` | Largest HTTP request to the discovery endpoints. |
| `--log-level <LEVEL>` | `info` | `debug`, `info`, `warn`, `error`, `fatal`. |
| `--parent-pid <PID>` | | Exit when that process exits. The SDK's `launch()` sets it; a service does not need it. |

Identity and behaviour flags apply to every connection: `--proxy`, `--proxy-bypass` ([Proxies, user agents, headers](/scraping/identity)), `--local-storage` ([Stored profiles](/scraping/profiles#the-same-localstorage-for-every-context-serve-local-storage)), `--fork` and `--prepare` ([Profiles booted once](/scraping/fork-profiles)), `--hyphen-digit-breaks` and `--render-prewarm` ([Screenshots and PDFs](/scraping/pdf)). `pyro serve --help` lists them all, and [serve options](/reference/serve) explains each. Several also read an environment variable, for launchers that cannot pass flags: `PYRO_FORK=1`, `PYRO_LOCAL_STORAGE`, `PYRO_PREPARE`, `PYRO_HYPHEN_DIGIT_BREAKS` and others (see [Environment variables](/reference/env)). The proxy and the network flags have no environment variable.

`--experimental-features` and `--load-resources` are accepted for compatibility with Lightpanda's command line and have no effect.

The server stops on `SIGTERM` or `SIGINT`. Closing it closes every connection; clients see the WebSocket drop. In fork mode it removes its template sockets on the way out.

## Getting the binary onto a server

The npm package carries the browser for each platform under `native/<platform>/`:

```
node_modules/@povio/pyro/native/
  linux-x64/    pyro, lib/ (ICU)
  linux-arm64/  pyro, lib/
  darwin-arm64/ pyro
  darwin-x64/   pyro
```

The patched JavaScriptCore is linked into the `pyro` binary. On Linux, keep `pyro` and its `lib/` directory together; the binary loads the bundled ICU libraries from there. You can run it directly, without Node.js, or through `npx pyro`, which spawns the same binary and forwards signals to it.

On Linux it needs glibc 2.39 or newer (for example Ubuntu 24.04) and these system libraries: `libstdc++6`, `libatomic1` (x64; harmless on arm64) and `libfontconfig1`, plus some fonts for screenshots and PDFs. It does not need a display server or a GPU. HTTPS uses built-in Mozilla root certificates, so no `ca-certificates` package is needed for the browser itself.

## systemd (Ubuntu 24.04)

Install the package into a fixed directory and create a system user:

```sh
sudo apt-get install -y nodejs npm libatomic1 libfontconfig1 fonts-dejavu-core fonts-liberation
sudo mkdir -p /opt/pyro && cd /opt/pyro
sudo npm install --omit=dev @povio/pyro@X.Y.Z
sudo useradd --system --home-dir /nonexistent --shell /usr/sbin/nologin pyro
```

Ubuntu's own Node.js is only needed to run `npm install` here; the service runs the binary directly. (`npm` may warn that the package wants Node.js 22; that applies to the JavaScript API, not to installing the binary.)

`/etc/systemd/system/pyro.service`:

```ini
[Unit]
Description=Pyro headless browser (CDP)
After=network-online.target
Wants=network-online.target

[Service]
User=pyro
Group=pyro
# linux-arm64 on ARM machines
ExecStart=/opt/pyro/node_modules/@povio/pyro/native/linux-x64/pyro \
  serve --host 127.0.0.1 --port 9222 --cdp-max-connections 32 --log-level info
Restart=always
RestartSec=2
# Pages hold memory while open; cap it so a runaway job cannot take the machine.
MemoryMax=4G
TasksMax=4096
LimitNOFILE=65536
Environment=TZ=UTC

NoNewPrivileges=yes
PrivateTmp=yes
ProtectSystem=strict
ProtectHome=yes
ProtectKernelTunables=yes
ProtectControlGroups=yes
RestrictSUIDSGID=yes
# Writable directory for downloads saved by `download({ saveTo })` steps, if you use them.
StateDirectory=pyro
# Do not set MemoryDenyWriteExecute=yes: JavaScriptCore's JIT needs writable, executable memory.

[Install]
WantedBy=multi-user.target
```

```sh
sudo systemctl daemon-reload
sudo systemctl enable --now pyro
journalctl -u pyro -f
```

`Restart=always` brings the server back after a crash or an out-of-memory kill. Clients must reconnect then: connections, contexts, pages and fork profiles do not survive a restart. Make your jobs reconnect and re-register profiles on failure. Fork mode keeps its template sockets in the temporary directory, which `PrivateTmp=yes` makes private to the service; that is fine.

`TZ` sets the time zone pages see by default (see [Time zone](/scraping/identity#time-zone)).

## Docker

A two-stage image: the first stage fetches the npm package, the second is Ubuntu 24.04 with only what the browser loads.

```dockerfile
# Stage 1: fetch the npm package, which bundles the Linux browsers.
FROM oven/bun:1 AS fetch
WORKDIR /pkg
RUN bun add @povio/pyro@X.Y.Z
RUN cp -r "node_modules/@povio/pyro/native/linux-$(uname -m | sed 's/x86_64/x64/;s/aarch64/arm64/')" /pyro

# Stage 2: Ubuntu 24.04 with the browser's libraries and some fonts.
FROM ubuntu:24.04
RUN apt-get update \
 && apt-get install -y --no-install-recommends libatomic1 libfontconfig1 fonts-dejavu-core fonts-liberation curl \
 && rm -rf /var/lib/apt/lists/*
COPY --from=fetch /pyro /opt/pyro
RUN useradd --system --uid 10001 --home-dir /nonexistent --shell /usr/sbin/nologin pyro
USER pyro
EXPOSE 9222
HEALTHCHECK --interval=30s --timeout=5s --retries=3 \
  CMD curl -fsS --max-time 4 http://127.0.0.1:9222/json/version > /dev/null || exit 1
ENTRYPOINT ["/opt/pyro/pyro", "serve", "--host", "0.0.0.0", "--port", "9222"]
```

```sh
docker build -t pyro .
docker run -d --name pyro --restart unless-stopped \
  -p 127.0.0.1:9222:9222 --memory 4g \
  pyro --advertise-host 127.0.0.1 --cdp-max-connections 32
```

Arguments after the image name are added to `serve`. This image was built and tested on Linux arm64 (Docker Desktop on Apple silicon): Playwright-library scripts on the host drove it, it printed PDFs with the DejaVu fonts, the health check reported `healthy`, and `--fork` served forked signed-in pages.

`--host 0.0.0.0` is needed inside the container so the published port reaches the server. Publish it on `127.0.0.1` of the host, as above, unless the host's network is private.

### The `Host` header in container networks

The server answers only requests whose `Host` header is an IP address or `localhost`, to block DNS rebinding attacks. In Docker Compose or Kubernetes, a client that connects to `ws://pyro:9222/` by service name gets `403 Forbidden`. Options:

- Resolve the name to an IP in the client and connect to `ws://<ip>:9222/`.
- Run the scraper and the browser in one pod or network namespace and connect to `127.0.0.1`.
- Put a proxy in front that sets `Host: 127.0.0.1:9222`.

Also set `--advertise-host` to an address clients can use (an IP), because `chromium.connectOverCDP('http://...')` and `puppeteer.connect({ browserURL })` read the WebSocket URL from `/json/version`. Or skip the lookup and pass the WebSocket URL directly.

## Health checks

```sh
curl -fsS --max-time 4 http://127.0.0.1:9222/json/version
```

`200` with JSON means the server accepts connections. Two caveats:

- At `--cdp-max-connections` live connections, the server stops accepting, and the health check waits like any client. Keep the limit above your peak, or treat a slow answer as "busy" rather than "dead" (a check with `--retries` gives it time).
- `/json/version` does not start a browser thread. For a deeper check, connect and load a page:

  ```ts
  import { chromium } from '@povio/pyro/wright';

  const browser = await chromium.connectOverCDP('ws://127.0.0.1:9222/');
  const page = await browser.newPage();
  await page.setContent('<p id="ok">ok</p>');
  if ((await page.locator('#ok').textContent()) !== 'ok') process.exit(1);
  await browser.close();
  ```

## Logging

The server writes a few lines to stderr: what it serves at startup, the listening address, and errors. For example:

```
pyro: localStorage preload for https://app.example.com: 2 entries
pyro: CDP server listening on ws://127.0.0.1:9222/
```

In fork mode the template reports failing profile `setup` steps there too. `--log-level warn` drops the startup lines. The server does not log requests, page console output or page errors; collect those in your client (`page.on('console')`, `page.on('pageerror')`, `page.on('requestfailed')`) and log them with the job.

## Securing it

CDP has no authentication. Whoever can open a WebSocket to the port can drive the browser: read every cookie and stored session of every context, load internal URLs from the server's network position, read files the server can read through upload steps, and use your proxies. Treat the port like a root shell.

- **Never expose it to the internet.** Bind to `127.0.0.1` (the default). If other machines must connect, bind to a private interface, and firewall the port to the hosts that need it.
- **Reach it over SSH** from your laptop or another server:

  ```sh
  ssh -N -L 9222:127.0.0.1:9222 scraper@browser-host
  # then connect to ws://127.0.0.1:9222/ locally
  ```

- **Or over a private network**: a VPN such as WireGuard or Tailscale, a VPC, or a Kubernetes network policy that admits only the scraper pods.
- **Separate tenants.** Contexts isolate jobs from each other, but not from someone holding the connection. Run one server per trust boundary.
- **Restrict what the browser can reach.** A page can request any URL the server can, including internal services and cloud metadata endpoints (`169.254.169.254`). Use egress firewall rules, or force traffic through a proxy (`--proxy`) that allows only the sites you scrape.
- **Run as an unprivileged user**, as both examples above do.

The server's own checks (the `Host` header must be an IP or `localhost`, WebSocket upgrades with an `Origin` header are refused) stop web pages in a browser from reaching it. They are not access control.

## License

Pyro is AGPL-3.0-only, a derivative of Lightpanda; the package includes `LICENSE` and `NOTICE`. The facts that matter for a service:

- You may run it, including as part of a commercial service.
- If you **modify** Pyro and let users interact with the modified program over a network, AGPL section 13 requires you to offer those users the corresponding source code of your version.
- If you **distribute** it (in an image you ship to others, for example), the usual GPL conditions apply: pass on the license and the source, or an offer of it.
- Your scraping scripts are separate programs that talk to the browser over the CDP protocol.

This is a summary, not legal advice. Read the license text, and ask a lawyer about your own case.
