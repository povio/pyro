# Vendored Mock Service Worker script

`mockServiceWorker.js` is the unmodified worker script of the
[msw](https://github.com/mswjs/msw) npm package, version 2.15.0
(`msw/lib/mockServiceWorker.js`), MIT licensed (Copyright (c) 2018–present
Artem Zakharchenko). `tests/service_worker.rs` serves it to prove that MSW's
real worker runs in lightningpanda; `tests/e2e/msw.mjs` drives the same flow
with the msw library through Playwright.
