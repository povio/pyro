# wpt-results

WPT results of pyro's main builds, for the docs site's status pages (/status/wpt.html). Written by
bench/wpt/status-export.mjs on main: the WPT workflow's `status` job adds runs/<sha>.json.gz and updates index.json
after each push to main whose gates passed. History before that was backfilled from the baselines' git log.

- runs/<sha>.json.gz: per kind (testharness, reftest, crashtest), per page: [status, passed, total, failing names?]
- index.json: the runs, oldest first, with totals per kind and per top-level directory
  ([tests in WPT, run, passed, subtests passed, subtests])
- manifest/<rev>.json.gz: every test URL in WPT's manifest at that revision, per kind (the denominator)
- chrome/<rev>.json.gz: Chrome stable's results on wpt.fyi nearest to that revision, per URL ([passed, total, status])
