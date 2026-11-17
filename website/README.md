# Pyro documentation site

VitePress site for Pyro (npm @povio/pyro; up to 0.0.33 @povio/lightning-panda), hosted on Robodev Lite
(project `prj-pqvevmsxr7el` in the Povio organization):
https://pyro.robodev.povio.dev (also https://prj-pqvevmsxr7el.robodev.povio.dev)

```sh
bun install
bun run dev       # http://localhost:5173
bun run build     # writes dist/; fails on dead internal links
bun run deploy    # robodev deploy: builds and uploads dist/ (needs `bunx robodev auth` or ROBODEV_DEPLOY_KEY)
```

- Pages live in `docs/<section>/*.md`; the sidebar is in `docs/.vitepress/config.ts`.
- Links are `.html` URLs (`cleanUrls: false`) and sections have no `index.md`: the
  Robodev host serves `.html` files as they are but answers extensionless paths
  with the root page and `/dir/` with a 404.
- `robodev/database.ts` declares one placeholder table because Robodev requires a
  database; the site itself is static. `robodev/.robodev` holds the project link
  (no secrets). Never commit a deploy key.
- `.github/workflows/docs.yml` builds on pull requests and deploys on main when
  the `ROBODEV_DEPLOY_KEY` secret is set.
- Document what the published package does. When main is ahead of the latest
  release, mark newer behaviour as such.

## WPT status pages

`/status/wpt.html` and `/status/wpt/<dir>.html` show every main build's WPT results (testharness, reftests,
crashtests) against all of WPT at the pinned revision, with Chrome stable's results beside them. `bun run build`
generates them from the `wpt-results` branch (`scripts/wpt-status.mjs`, fetched from origin, or `WPT_RESULTS_DIR=<checkout>`;
without it the page is a placeholder): one VitePress page, `docs/status/wpt.md`, a JSON file per directory under
`docs/public/status-data/`, and after the build a copy of that page's HTML per directory (a VitePress page each would
carry the site's page map, ~500 MB in all). The data's format and how it is written: `bench/wpt/status-export.mjs`.
