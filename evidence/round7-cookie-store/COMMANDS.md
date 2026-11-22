# Matched commands

The exact source and executable identities are in provenance.json. The base was built by the editing lane on the same machine. The candidate was built separately with the CI JavaScriptCore archive selected by scripts/jsc/cache-key.sh, a static archive digest of 3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029, the wptiter profile inheriting release-fast, one Cargo job, render enabled, no LTO and 16 codegen units.

    python3 run.py base
    node probe.cjs base ./base-pyro
    node probe.cjs chrome158
    node partitioned-probe.cjs chrome158
    python3 run.py candidate
    node probe.cjs candidate ./pyro
    node partitioned-probe.cjs candidate ./pyro
    python3 report.py
    python3 ledgers.py
    python3 changed.py
    python3 audit.py
    ./native.sh

The WPT runner uses a fresh page context, four page jobs, wptserve, testdriver, HTTP/2 and the pinned generated-global inventory. It records the original named statuses including partial observations on timeouts. Strict pages require completion, an OK harness and all nonempty named results passing. The signed reference was extracted from its original shards. The direct signed controls are separate executions of the same Chrome stable binary.

Rows in named.jsonl.gz consist of URL, canonical test name, one-based occurrence, then base, candidate and signed status. A null cell is unobserved. API and legacy subsets are also included separately. scope-scores.json retains the lifecycle, error text and named denominator of every non-OK page.
