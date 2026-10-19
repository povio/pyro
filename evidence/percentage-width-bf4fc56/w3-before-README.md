# Office badge width failure

Original frozen failure: `f9729e06b9defe7adbc4b44267fadcb8c01bc6e82eb9486e0226c10a2fa2d837`, lists seeded offices as rendered table rows. The original test expects both Business Partner and Office labels in the row; rewrite renders +2 instead.

## Actual application measurement

One original case per engine, exact frozen TOS e6cf6953 plus diagnostic-only measurement logging; original assertions unchanged. Rewrite engine fb3b12dc signed SHA5384b453. Both runs used isolated fresh databases, no retry, exact selected identity, external retained artifacts, and restored the pre-run database set. Rewrite failed the original label assertion; Chromium passed.

| Measurement | Chromium | Rewrite |
|---|---:|---:|
| Types TD width |320|320|
| Badge container width |311|5|
| Hidden measurement width |239.171875|20|
| Business Partner badge width |116.65625|5|
| Office badge width |49.8125|5|
| +1 / +2 widths |26.46875 / 28.234375|5 / 5|
| Container column gap |6|0 (normal)|

`BusinessPartnerTypeBadges` computes visibleCount from these widths. The rewrite measurements force zero visible labels and +2. The labels and seeded types remain correct; replacing label assertions would conceal native measurement behavior. Native ownership is W2; no native or application source repair is included here.

## Reduced static reproduction

The retained script isolates the same container/hidden-measurement algorithm with a 320px cell, percentage-width container, max-content hidden measurement row and four badges. Chromium computes container320 and visibleCount2; rewrite computes container5 and visibleCount0. This reproduction is not full CSS/flex/table equivalence and does not establish one exclusive parser/layout root cause. Actual and reduced geometry differ numerically, but both reproduce the count decision.

The script imports the same pinned local Playwright1.59.1 module; adapt only its module path on another host. Each runner owns and closes its browser/HTTP server. No credentials, request bodies or auth state are retained. The diagnostic patch records bounded geometry and computed properties, not row data.
