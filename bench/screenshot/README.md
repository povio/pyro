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

`repeat.ts` measures repeat screenshots of one page (no change, one text change, one class toggle), where the
renderer keeps the page's document and applies only changes, and DOM work and locator reads before and after a
page's first screenshot. `--pages <dir>` adds static page snapshots (one directory each: `page.html`, `meta.json` with `base_url` and
`resources` `{url, file}`, `res/<file>`), served with their sheets and resources:

```sh
bun bench/screenshot/repeat.ts --bin target/release/lightningpanda --iters 30
```
