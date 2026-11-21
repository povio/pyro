from pathlib import Path
import json,sys
p=Path(sys.argv[1]);G=lambda f:json.loads((p/f).read_bytes())
s=G('summary.json');h=G('historical-summary.json');stud=G('studio-css-candidate-summary.json');r=G('receipt-provenance.json')
def table(st):
 def n(k):return format(st.get(k,0),',d')
 return ' | '.join([n('planned'),n('base/passes'),n('head/passes'),n('base-to-head/gainedPass'),n('base-to-head/lostPass'),n('base/fullPages')+' → '+n('head/fullPages'),n('chrome-to-head/pairedPages'),n('chrome-to-head/gainedPass'),n('chrome-to-head/lostPass')])
def history(sc,lab):
 st=h[sc][lab];return ' / '.join(str(st.get(k,0)) for k in ('Pass','Fail','missing-name','unobserved-page'))
text="""# Combined browser conformance guard

LP cbdeb11931f74c3de4417a25fb9103f1df84d54e and b57c35dd45d54d6781aafcb2c8242c9ad9b26e04 were compared on the same 4,476 pinned URLs. 4,041 belong to the frozen Chrome 158 priority/calibration selection. The 435 supplemental URLs are disjoint from those 4,041, including five independently reviewed storage-only URLs outside the frozen eligible census. The full non-CSS census has 31,827 official variants, of which the frozen selection includes 30,359. The 1,091 CSS URLs are guards. The scores here apply only to the listed selections.

| Cohort | URLs | Base passes | Head passes | Gained | Lost | Fully passing LP pages | Paired with local Chrome | Head-only passes | Chrome-only passes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
"""
for co,lab in [('priority/non-css','Priority non-CSS'),('priority/css','Priority CSS'),('extra-feature/non-css','Ordinary additions'),('wasm-storage-supplement/non-css','Storage-only supplement')]:
 text+='| '+lab+' | '+table(s[co])+' |\n'
text+="""
A paired exclusive count requires both engines to report the URL. An unreported name on an observed page remains null. A strictly passing page requires a complete receipt, an OK harness and at least one name, with every name passing. The original harness error, timeout or not-run outcomes remain in the exported ledgers. effects.json gives area-level gains/losses, lost-names.jsonl.gz the full raw loss set, and summary.json the number of added and missing names. The historical ledger matches all original duplicate occurrences using authenticated WPT server port roles. Each row retains its original classification. Concurrent page timings and missing names can differ between runs.

| Historical group | Base pass / fail / missing / withheld | Head pass / fail / missing / withheld | Chrome pass / fail / missing / withheld |
| --- | --- | --- | --- |
"""
for sc,lab in [('all','Earlier 292'),('referrer','157 referrer-policy subset')]:
 text+='| '+lab+' | '+' | '.join(history(sc,x) for x in ('base','head','chrome'))+' |\n'
text+=f"""
## Browser provenance

WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603, manifest SHA-256 34a67512a84d43630f7adbecebd4378df5f410446591015cc6a20a3034e02819, pinned CI JSC 0e0dd7038a7a-6f4649e395e0. Both LP binaries and the canonical local reference used the same manifest runner, 800×600 DPR 1, four jobs, fresh page contexts, testdriver, HTTP/2 and wptserve. Source-audit.json records both LP executable hashes and the 538 identical Cargo package identities. The new direct dependency edges are rustfft for media convolution and ring for Web Crypto. The Wasm backend and WPT expectations are untouched.

Local Google Chrome stable 158.0.8127.37 on macOS 27.2.1 arm64 used the signed Chrome image, fixed WPT profile, SPKI and Playwright 1.65.1. The census receipt takes precedence on overlaps. Eligible additional four-job feature-team receipts from this same launcher fill missing local stable rows. receipt-provenance.json marks the standalone sources. The historical 157 and public references retain their own origin and are excluded from these counts. Each baseline, candidate and Chrome receipt has a SHA-256 and producing server mapping. The tables preserve original names before authenticated port normalization. Name collisions cause the exporter to fail.

The separately recorded Mac Studio Chrome stable 158.0.8127.37 reference produced {stud['studioPass']:,} named passes and {stud['studioFull']} fully passing pages across the {stud['paired']:,} candidate-paired CSS URLs. LP shared {stud['commonPass']:,} passing names with Studio, with {stud['headOnlyPass']:,} LP-only and {stud['studioOnlyPass']:,} Studio-only. studio-css-candidate-* preserves that independent comparison. The frozen CSS inventory contains 8,162 official URLs, including 8,123 Studio observations and 39 exclusions. Studio used headless CDP and 16 shards; four used extension switches, and the service-worker supplement used the WPT leaf SPKI. Other browser switches, driver and concurrency details are incompletely documented there. The MacBook and Studio references remain separate. [Frozen cross-machine and eligibility records](https://github.com/povio/pyro/tree/f6149854f6bd023cbfd95e0b03f75c79d509bac8/evidence/round6-chrome158)

## Reproduction

From the evidence directory run `shasum -a 256 -c SHA256SUMS` and `python3 verify.py .`. The offline verifier reconstructs the complete per-cohort score tables, status totals, duplicate counts, strict pages and historical distributions. Named rows are [URL, canonical name, one-based occurrence, base, head, local Chrome], with each observed name carrying [status, original name]. Page rows include the URL, task, cohort, scope, source/blob/environment/protocol and three browser observations [page status, harness status, strict pass, passing names, reported names, producing task]. Null marks an absent observation. Studio tables use the separate source's numeric harness code and original [reported names, passing names] field order; the candidate and MacBook fields retain the local format.

The original run command is in run.py. To regenerate from original local receipts, set LP_CACHE to the Lightningpanda cache root, run `python3 export.py OUT`, `python3 effects.py OUT`, and `python3 studio.py OUT "$LP_CACHE/round6-reference-158/census/export-priority"`. The candidate snapshot uses the complete task list in tasks.json, exact URL-to-task associations in selected-urls.json and original immutable blob identities in the page rows. No source or expectation file is part of this evidence commit.
"""
(p/'README.md').write_text(text)
