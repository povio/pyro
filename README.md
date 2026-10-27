# lightningpanda

A Rust rewrite of the [Lightpanda](https://github.com/lightpanda-io/browser) headless browser on
**JavaScriptCore**. It never uses V8. It targets fast, low-memory browser E2E testing over CDP.

**Status: early bootstrap, not a usable browser yet.** The JavaScriptCore runtime, the event loop and the
page/realm lifecycle are real and tested. The DOM, networking, CDP and document loading are in progress.
[docs/PARITY.md](docs/PARITY.md) lists what exists and what is missing. [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)
describes the module contract.

## Build (macOS)

```sh
rustup toolchain install stable
cargo build --release
cargo test
./scripts/check-no-v8.sh
```

On macOS every linked executable (`lightningpanda`, the test binaries) is ad hoc re-signed with
`com.apple.security.cs.allow-jit` and nothing else; this runs through `scripts/macos/link-and-sign.sh`, which
`.cargo/config.toml` sets as the linker. Without that entitlement the system JavaScriptCore runs interpreter-only.
Cargo reads `.cargo/config.toml` from the current directory upwards, so run cargo from the repository: a build started
elsewhere (`cargo build --manifest-path ...`) links unsigned, and the `lightningpanda` build script warns about it. If a
binary is copied or re-signed outside Cargo, keep the entitlement:
`codesign -f -s - --entitlements scripts/macos/jit.entitlements <binary>`. With the JIT active, JSC's default
signal-based traps do not stop JIT-compiled loops, so `lp_js::configure_process` (called by `Engine::new` and at the
start of the CLI) sets `JSC_usePollingTraps=true` unless that variable is already set; script timeouts and termination
depend on it.

The release profile keeps `strip = "none"`: Cargo otherwise strips after linking when
`CARGO_PROFILE_RELEASE_DEBUG=0`, which removes the JIT entitlement added by the linker wrapper.
An explicit strip override requires re-signing the final binary afterward. Check this build contract with
`python3 scripts/macos/check-jit-signing.py` (macOS; compiles a small temporary executable).

## Usage

```sh
lightningpanda version
lightningpanda eval -e 'setTimeout(() => console.log("hi"), 10)'
lightningpanda eval script.js --timeout-ms 5000
lightningpanda serve --port 9222
# Test runs that need no front-end server and no per-context auth setup:
# serve a build directory in place of its origin and preload localStorage.
lightningpanda serve --static http://localhost:3000=/path/to/dist \
  --local-storage auth.json   # {"http://localhost:3000": {"token": "..."}}
```

`serve --help` lists every option. `--static` and `--local-storage` also read
`LIGHTNINGPANDA_STATIC` (entries separated by `;`) and `LIGHTNINGPANDA_LOCAL_STORAGE`
from the environment, for launchers that cannot pass flags.

To hide the app's own boot time as well, `--prepare http://localhost:3000/dashboard`
(env `LIGHTNINGPANDA_PREPARE`) keeps one page per connection booted at that URL
before any client connects and boots the next one whenever the thread is quiet.
A test takes it with the browser-level CDP command
`Lightningpanda.acquirePreparedPage {url}`: the page is announced like a new
target, so Playwright sees it as a page of its default context, already loaded.
`Lightningpanda.registerPreparedPage {url, initScripts, viewport, pool}` declares
a profile at run time, including the scripts a test would otherwise install
with `addInitScript`; `Lightningpanda.preparedPages` lists what is waiting.

## License

AGPL-3.0-only. This is a derivative of Lightpanda; see [NOTICE](NOTICE).
