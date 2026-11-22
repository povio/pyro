"""Generate BUILD-DAY.md from ledger.jsonl + estimates.json. Documentation only; nothing is built."""
import json, os, re, sys

TIP = sys.argv[1] if len(sys.argv) > 1 else "?"
L = [json.loads(l) for l in open("ledger.jsonl")]
by = {r["id"]: r for r in L}
est = json.load(open("estimates.json"))
sc = est["tiers"]["implemented"]["scope"]

out = []
w = out.append
w("# Round 7 batch-1000: build-day package")
w("")
w("Documentation only. **Do not build until the user authorizes it** (standing ZERO-compile order).")
w("")
w(f"- Batch branch: `origin/integration/round7-batch-1000` at `{TIP}` (when this file was generated).")
w(f"- Ledger: `origin/ledger/round7-fix-queue` (`ledger.jsonl`, `estimates.json`).")
w(f"- Expected, unverified: MacBook in-scope window pages implemented-unbuilt **{sc['macbook_in_scope']}**; "
  f"Studio **{sc['studio_in_scope']}**; MacBook worker variants (separate column) {sc['macbook_worker_variants_if_enabled']}.")
w("- Every figure is an estimate from source reading. Tentative pages are counted only where the owner gave a figure or the coordinator ruled.")
w("")
w("## 1. JSC archive")
w("")
w("Build the JSC archive through patch **0041**:")
w("")
w("| Patch | What | Batch commit |")
w("|---|---|---|")
w("| 0038 | inline module | (pre-batch archive) |")
w("| 0039 | module-fetch-referrer | 2dbda236a |")
w("| 0040 | deferred-work (async wasm, worker loops via c6295b18e) | a8c9d7350 |")
w("| 0041 | named-properties-chain | f47805459 + fix 92579f9cd |")
w("")
w("- The held HTMLCollection-delete patch (4f4684042, also numbered 0039) is **not** on this branch. No renumbering is needed for this build.")
w("- If global-lookup or realm-startup regressions appear, drop 0041 first.")
w("")
w("## 2. Pre-build branch hygiene")
w("")
w("- The evidence commit 5c95c55d1 (18 files under `evidence/round7-inline-module-base-url/`) is removed by **3047718ed**, which is an ancestor of the tip. Verified: 0 files under that path at the tip.")
w("- The integrator has the compile fix for 0206f4243 (frames.rs `let` in argument position). Confirm it before the first compile.")
w("- Run `git show --stat` and confirm the tip matches what is being built.")
w("")
w("## 3. Security smoke tests (mandatory)")
w("")
sec = [i for i in (by["POLICY-build-watchlist"]["items"] or [])
       if re.search(r"(?i)secur|taint|privacy|leak|cross-origin|SecurityError|smoke", i)]
for i in sec:
    w(f"- {i}")
w("")
w("## 4. Watchlist (behaviour changes and regression risks)")
w("")
for i in by["POLICY-build-watchlist"]["items"]:
    if i in sec:
        continue
    w(f"- {i}")
note = by["POLICY-build-watchlist"].get("note")
if note:
    w("")
    w("Additional watchlist notes:")
    for part in [p.strip() for p in note.split(" | ") if p.strip()]:
        w(f"- {part}")
w("")
w("## 5. Per-commit test checklist")
w("")
w("Taken from each ledger row's `review.build_checklist`.")
w("")
for r in L:
    rv = r.get("review") if isinstance(r.get("review"), dict) else None
    cl = rv.get("build_checklist") if rv else None
    if not cl:
        continue
    w(f"### {r['id']}: {r.get('title') or r.get('subject') or ''}")
    w(f"Commit: {r.get('commit')}")
    for c in (cl if isinstance(cl, list) else [cl]):
        w(f"- {c}")
    w("")
w("## 6. Guard lists (must not regress)")
w("")
G = os.path.expanduser("~/Library/Caches/lightningpanda")
for f in ["round7-local/history-nav-guards/iframe-or-traversal.txt",
          "round7-local/history-nav-guards/passing-today-c3372ad21.txt",
          "round7-c02-shadow-offcanvas/C15-shared-credited-71.json"]:
    p = os.path.join(G, f)
    n = "?"
    if os.path.exists(p):
        try:
            n = len(json.load(open(p))) if p.endswith(".json") else sum(1 for l in open(p) if l.strip())
        except Exception:
            pass
    w(f"- `~/Library/Caches/lightningpanda/{f}` ({n} entries)")
w("- F02 focusgroup: the 29 pages that already pass (census c3372ad21) are a regression watch.")
w("- WPT gates: full reftest gate, testharness gates, Linux CI.")
w("")
w("## 7. Rows to verify that are credited on source reading only")
w("")
for r in L:
    n = (r.get("note") or "")
    if r["state"] == "implemented-unbuilt" and ("covered" in r["id"].lower() or re.search(r"(?i)build-day verify|BUILD-DAY VERIFY|covered by", n)):
        w(f"- {r['id']}: {len(r.get('page_list') or [])} pages ({r.get('commit')})")
w("")
w("## 8. Excluded or held (not counted)")
w("")
w("- Mixed content: 93cf2f04c, reverted by 0729192ae. Fetch-metadata, permissions-policy, SRI, COOP/COEP, security taint pages.")
w("- Policy decisions pending: document.domain (13 pages), cross-origin exotic WindowProxy/Location (JSC), real back-forward cache.")
open("BUILD-DAY.md", "w").write("\n".join(out) + "\n")
print(len(out), "lines;", len(sec), "security items")
