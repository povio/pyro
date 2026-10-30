# Screenshot benchmark (end to end)

`match-screenshot.ts` measures screenshots through the real path: a `lightningpanda` binary built with
`--features render`, the fixtures served over HTTP by Bun (Lato as a web font, so the renderer fetches it through
the page's network), and the `lightningpanda-test` client over CDP.

```sh
cargo build --release --features render
bun bench/screenshot/match-screenshot.ts --bin target/release/lightningpanda --iters 50
```

Fixtures (copied from the prototype on `proto/blitz-screenshot`): `login` (flex-centered card, gradient,
box-shadow), `dashboard` (flex header, 4-column grid, 300-row table), `article` (about 3k words). Viewport
1280x720; the dashboard also at DPR 2. See [docs/automation/SCREENSHOTS.md](../../docs/automation/SCREENSHOTS.md).
