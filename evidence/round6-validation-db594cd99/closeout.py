from pathlib import Path
import collections,gzip,hashlib,json,shutil
p=Path(__file__).resolve().parent
s=json.loads((p/'summary.json').read_bytes())
assert len(s)==11
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
  assert r[0] not in pages;pages[r[0]]=[k,*r[1:]]
 for key in ('old','new','gains','losses','count','oldFull','newFull','restarts','watchdog'):tot[key]+=x[key]
 for f in (named,pg,p/(k+'.selection.json')):shutil.copy2(f,o/f.name)
 with gzip.open(o/(k+'.receipt.json.gz'),'wb') as dest:dest.write((p/(k+'.json')).read_bytes())
 text.append(f"| {k} | {x['count']} | {x['old']} → {x['new']} | +{x['gains']} / −{x['losses']} | {x['oldFull']} → {x['newFull']} |")
assert len(pages)==tot['count']==734
(o/'changes.json').write_text(json.dumps(changes,indent=1)+'\n')
(o/'totals.json').write_text(json.dumps(tot,indent=1)+'\n')
for k in ('summary.json','omissions.json','parser-controls.json','pixel-result.txt','guards.py','closeout.py'):shutil.copy2(p/k,o/k)
(o/'README.md').write_text("""# Combined-head integration guards

Measured detached commit db594cd9998cf18c17e0258bd433b646edf705de from a fresh Cargo target on macOS arm64 with CI JSC 0e0dd7038a7a-6f4649e395e0. The static JSC SHA-256 is 3deb71f8e7d14daf174e35340a4ff0c1900b0e9f576503adeb5facbb1201f029. The wptiter executable SHA-256 is b643274be2c4733e9eb56af92e7f024cfb0b0ff19e24cf7d7b8585151f31fd8e. WPT revision is a421f2fea5eb7f91e0db399e44b0fb0779ee1603.

Prior results are reused LP receipts. Focus, guard, navigator, production and CSS layout shards compare to 97a4b379d (evidence 1e3f7d8e9). The two VideoFrame shards compare to 678808573 (evidence 89d558209). CSS Values compares to the 21fd48c15 candidate receipt committed at 3e7072d3b. Matched URL, name and occurrence sets are retained; per-cohort strict page results and raw inputs include harness errors, timeouts and missing results. Seven Studio exclusions and two interrupted-shard URLs were withheld from the 236-page CSS Values selection. The 58 production URLs retain the separately verified Wasm storage control.

| Guard | URLs | Prior → combined named passes | Gains / losses | Prior → combined fully passing URLs |
| --- | ---: | ---: | ---: | ---: |
"""+'\n'.join(text)+"""

All six DOMParser lifecycle observations matched the saved Chrome 158 controls. The browser-executed VideoFrame pixel, layout, clone, close, transfer and color-space assertions passed. Commands used the pinned build-exact helper with a fresh target, the no-native-retry WPT runner and original worker counts and flags recorded in guards.py, followed by closeout.py. Each receipt hash and prior receipt hash is in summary.json. This validation measures the specified LP head against its feature precursors.
""")
print(dict(tot))
