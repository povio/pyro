# Lightningpanda documentation site

VitePress site for Lightningpanda, hosted on Robodev Lite
(project `prj-pqvevmsxr7el` in the Povio organization):
https://prj-pqvevmsxr7el.robodev.povio.dev

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
