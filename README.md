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

## Usage

```sh
lightningpanda version
lightningpanda eval -e 'setTimeout(() => console.log("hi"), 10)'
lightningpanda eval script.js --timeout-ms 5000
```

## License

AGPL-3.0-only. This is a derivative of Lightpanda; see [NOTICE](NOTICE).
