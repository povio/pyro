from pathlib import Path
import collections,gzip,hashlib,json,shutil
p=Path(__file__).resolve().parent
s=json.loads((p/'summary.json').read_bytes())
assert len(s)==12
o=p/'evidence';o.mkdir(exist_ok=True)
changes=[];pages={};tot=collections.Counter();text=[]
for k,x in s.items():
 named=p/(k+'.named.jsonl.gz');pg=p/(k+'.pages.json')
 ct=collections.Counter()
 with gzip.open(named,'rt') as f:
  for line in f:
   u,n,i,a,b=json.loads(line);old=bool(a and a[0]=='Pass');new=bool(b and b[0]=='Pass')
   ct['old']+=old;ct['new']+=new;ct['gains']+=new and not old;ct['losses']+=old and not new
   if new!=old:changes.append([k,u,n,i,a,b])
 for key in ('old','new','gains','losses'):assert x[key]==ct[key]
 rows=json.loads(pg.read_bytes());assert sum(z[3] for z in rows)==x['oldFull'] and sum(z[4] for z in rows)==x['newFull']
 for r in rows:
  pages.setdefault(r[0],[]).append([k,*r[1:]])
 for key in ('old','new','gains','losses','count','oldFull','newFull','restarts','watchdog'):tot[key]+=x[key]
 for f in (named,pg,p/(k+'.selection.json')):shutil.copy2(f,o/f.name)
 with gzip.open(o/(k+'.receipt.json.gz'),'wb') as dest:dest.write((p/(k+'.json')).read_bytes())
 text.append(f"| {k} | {x['count']} | {x['old']} → {x['new']} | +{x['gains']} / −{x['losses']} | {x['oldFull']} → {x['newFull']} |")
assert len(pages)==760 and tot['count']==765
(o/'changes.json').write_text(json.dumps(changes,indent=1)+'\n')
(o/'totals.json').write_text(json.dumps(tot,indent=1)+'\n')
for k in ('summary.json','omissions.json','parser-controls.json','pixel-result.txt','guards.py','closeout.py'):shutil.copy2(p/k,o/k)
(o/'README.md').write_text("""# Combined-head integration guards

Measured detached commit 305840f45afbc4f55bfa24f303c1e858d30e4c1d from a fresh Cargo target on macOS arm64 with CI JSC 0e0dd7038a7a-6f4649e395e0. The static JSC SHA-256 is 3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029. The wptiter executable SHA-256 is BINARY_SHA. WPT revision is a421f2fea5eb7f91e0db399e44b0fb0779ee1603.

Prior results are reused LP receipts. Focus, guard, navigator, production and CSS layout shards compare to 97a4b379d (evidence 1e3f7d8e9). The two VideoFrame shards compare to 678808573 (evidence 89d558209). CSS Values compares to the 21fd48c15 candidate receipt committed at 3e7072d3b. Matched URL, name and occurrence sets are retained; per-cohort strict page results and raw inputs include harness errors, timeouts and missing results. Seven Studio exclusions and two interrupted-shard URLs were withheld from the 236-page CSS Values selection. The 58 production URLs retain the separately verified Wasm storage control.

| Guard | URLs | Prior → combined named passes | Gains / losses | Prior → combined fully passing URLs |
| --- | ---: | ---: | ---: | ---: |
"""+'\n'.join(text)+"""

All six DOMParser lifecycle observations matched the saved Chrome 158 controls. The browser-executed VideoFrame pixel, layout, clone, close, transfer and color-space assertions passed. Commands used the pinned build-exact helper with a fresh target, the no-native-retry WPT runner and original worker counts and flags recorded in guards.py, followed by closeout.py. Each receipt hash and prior receipt hash is in summary.json. This validation measures the specified LP head against its feature precursors.
""")
r=o/'README.md';r.write_text(r.read_text().replace('BINARY_SHA',(p/'binary.sha256').read_text().split()[0])+'\nThe 31 Studio closeout observations compare against the saved b2a76c024 cumulative receipts for root viewport cycles and the CSS namespace. Five URLs overlap the CSS Values cohort.\n');shutil.copy2(p/'studio-focus-reference.json',o/'studio-focus-reference.json');print(dict(tot))
