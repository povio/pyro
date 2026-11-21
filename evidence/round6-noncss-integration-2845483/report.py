"""Render immutable integration evidence totals and reference provenance."""
import collections,json,sys,gzip,subprocess,shutil
from pathlib import Path
p=Path(sys.argv[1]);G=lambda n:json.loads((p/n).read_bytes());st=G('summary.json');hist=G('historical-summary.json');source=G('source-audit.json');studio=G('studio-css-candidate-summary.json')
focus=Path(__file__).resolve().parent/'focus.py'
subprocess.run([sys.executable,str(focus),str(p)],check=True);shutil.copy2(focus,p/'focus.py')
fc=G('prior-focus-summary.json')['counts'];assert fc['paired']==152
def n(x):return f'{x:,}'
def hg(x):return ' / '.join(str(x.get(k,0)) for k in ('Pass','Fail','missing-name','unobserved-page'))
s=f"""# Round 6 non-CSS integration

The verified LP replay is {source['head']} on {source['base']}. Both binaries were freshly measured on 4,929 identical upstream WPT URLs in 58 disjoint shards. The comparison includes the original 4,041 priority/calibration controls, 430 feature supplements, 406 second-wave controls, 40 upstream CSS controls, and five storage plus seven metadata variants from separate reviewed overlays. The full non-CSS inventory has 31,827 URLs, with 30,359 in the frozen ordinary run. The overlays keep their original exclusion reasons. Neither their results nor the CSS controls change that denominator.

| Selection | URLs | Base named | Replay named | Gained / lost | Base / replay full pages | Local stable paired | Replay-only / stable-only passes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
"""
total=collections.Counter();plot=[]
for k,lab in [('priority/non-css','Priority non-CSS'),('priority/css','Priority CSS'),('extra-feature/non-css','First feature supplement'),('additional-noncss/non-css','Second feature supplement'),('upstream-css/css','Additional upstream CSS'),('wasm-storage-supplement/non-css','Wasm storage overlay'),('wasm-metadata/non-css','Wasm metadata overlay')]:
 c=st[k];total.update(c);plot.append((lab,c))
 def fmt(x):return n(c.get(x,0))
 txt=lambda a,b:fmt(a)+' / '+fmt(b)
 s+='| '+' | '.join([lab,fmt('planned'),fmt('base/passes'),fmt('head/passes'),txt('base-to-head/gainedPass','base-to-head/lostPass'),txt('base/fullPages','head/fullPages'),fmt('chrome-to-head/pairedPages'),txt('chrome-to-head/gainedPass','chrome-to-head/lostPass')])+' |\n'
s+=f"""
Across the 4,929 selected URLs, LP gained {n(total['base-to-head/gainedPass'])} named passes and lost {n(total['base-to-head/lostPass'])}. Fully passing pages went from {n(total['base/fullPages'])} to {n(total['head/fullPages'])}. The local stable pass sets are paired on {n(total['chrome-to-head/pairedPages'])} URLs. Unmeasured local stable pages remain null. The largest changes, raw lost cases, new/missing names and harness transitions are recorded in effects.json, lost-names.jsonl.gz and summary.json.

## Reconciliation and controls

The 22 source commits after aa0268b47 through 6c6045cfe replay in the same order. All 130 upstream-only paths and 50 topic-only paths retain their original blobs. Applying the original topic diff independently to the three shared upstream files reproduces the replay blobs. source-audit.json records the prior and replay patch IDs, binary fingerprints and the {source['lockedPackageIdentities']} unchanged resolved package identities. The WPT expectations and shipped Wasm backend are untouched.

The separately measured original 6c6045cfe topic and integrated replay were paired on 152 late-feature URLs. The replay gained {n(fc['namedGains'])} and lost {n(fc['namedLosses'])} named passes, with {fc['priorFull']} and {fc['headFull']} strict pages respectively. The exact old-topic comparand, named outcomes and receipts are in prior-focus-*.

The independent Mac Studio Chrome 158 CSS reference covers all {n(studio['paired'])} CSS controls. Studio fully passed {n(studio['studioFull'])} pages and passed {n(studio['studioPass'])} names, with {n(studio['commonPass'])} names passing on both the replay and Studio, {n(studio['headOnlyPass'])} only on LP and {n(studio['studioOnlyPass'])} only on Studio. studio-css-candidate-pages.jsonl.gz and studio-css-candidate-names.jsonl.gz preserve the separate machine columns. The local reference is the primary comparator when present. Studio used CDP, disposable profiles and 16 original shards, with changed extension switches on four shards and the WPT leaf SPKI on the 41-variant service-worker supplement. Other switch, concurrency and driver details are not fully recorded. [Independent signed Chrome 158 CSS reference](https://github.com/povio/pyro/tree/f6149854f6bd023cbfd95e0b03f75c79d509bac8/evidence/round6-chrome158)

| Historical occurrences | Base pass / fail / missing / withheld | Replay pass / fail / missing / withheld | Chrome 158 pass / fail / missing / withheld |
| --- | --- | --- | --- |
"""
for k,lab in [('all','Earlier 292'),('referrer','157 referrer-policy subset')]:s+='| '+lab+' | '+' | '.join(hg(hist[k][l]) for l in ('base','head','chrome'))+' |\n'
s+=f"""
Original historical classifications and individual outcomes are in historical-292.json. A timeout retains its reported named outcomes. The MacBook and Mac Studio columns remain separate; no Chrome 157 or development-channel measurement enters these totals.

## Reproduction and data

The local runs used WPT a421f2fea5eb7f91e0db399e44b0fb0779ee1603 (manifest SHA-256 34a67512a84d43630f7adbecebd4378df5f410446591015cc6a20a3034e02819), CI JSC {source['jsc']}, wptserve with HTTP/2, testdriver, fresh contexts, manifest-generated globals, four jobs, 800×600 at DPR 1. The MacBook local reference is Google Chrome stable 158.0.8127.37 on macOS 27.2.1 arm64, with verified Google signature, Playwright 1.65.1 and the recorded WPT stable profile. The export uses complete census receipts first, then individually verified four-job same-profile receipts for matching missing URLs. Each observed source blob, environment, protocol, original name, duplicate occurrence and subtest status is retained. Selection hashes and raw receipt/log/config hashes are included. The upstream source selection is held fixed.

Run `shasum -a 256 -c SHA256SUMS` followed by `python3 verify.py .` from the evidence directory. This rechecks page and named pass sets, both directed comparisons, strict pages, missing rows, historical distributions and original duplicate order. The acquisition command is in run.py; export.py, effects.py, studio.py and audit-source.py document the original-cache rebuild. The selected URL-to-shard map and task digests are in selected-urls.json and tasks.json.

Each named row is [URL, canonical name, one-based occurrence, base, replay, local Chrome]. Browser fields are [status, original name] or null. Page fields include source/blob/environment/protocol, then [page status, harness status, strict pass, passing names, reported names, producing shard] for each browser. A strict pass requires a complete page, an OK harness, and at least one subtest with every subtest passing. Studio preserves its own raw numeric harness status and original [reported names, passing names] order. Original wire ports are canonicalized only with the producing WPT server configuration. Unattributed native events and earlier exclusions remain coverage gaps.
"""
(p/'README.md').write_text(s)

max_gain=max((c.get('base-to-head/gainedPass',0) for _,c in plot),default=1) or 1
max_loss=max((c.get('base-to-head/lostPass',0) for _,c in plot),default=1) or 1
bars=''.join(f'<div>{lab} <strong>+{n(c.get("base-to-head/gainedPass",0))} / −{n(c.get("base-to-head/lostPass",0))}</strong><span style="width:{c.get("base-to-head/gainedPass",0)/max_gain*100:.2f}%;background:var(--chart-1)"></span><span style="width:{c.get("base-to-head/lostPass",0)/max_gain*100:.2f}%;background:var(--chart-2)"></span></div>' for lab,c in plot)
rows=''.join(f'<tr><td>{lab}</td><td>{n(c["planned"])}</td><td>{n(c.get("base-to-head/gainedPass",0))}</td><td>{n(c.get("base-to-head/lostPass",0))}</td><td>{n(c["base/fullPages"])} → {n(c["head/fullPages"])}</td></tr>' for lab,c in plot)
(p/'comparison.html').write_text('<!doctype html><meta charset="utf-8"><style>div{margin:7px 0}div span{height:4px;display:block}strong{font-weight:500}table{width:100%;border-collapse:collapse;font:14px var(--font-sans,system-ui)}th,td{text-align:right;padding:6px 1px;border-bottom:1px solid var(--border)}th:first-child,td:first-child{text-align:left}caption{text-align:left;font-weight:600;margin-bottom:9px}</style><table><caption>Matched integration guard</caption><thead><tr><th>Selection</th><th>URLs</th><th>Gained</th><th>Lost</th><th>Full pages</th></tr></thead><tbody>'+rows+'</tbody></table><section>'+bars+'</section>')
