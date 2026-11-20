from pathlib import Path
import json,sys,collections,gzip
p=Path(__file__).resolve().parent; d=p.parent;g=d/'css-reconciliation-572c'
assert (g/'platform-extra-final.json').is_file()
import importlib.util
spec=importlib.util.spec_from_file_location('csscompare',g/'compare.py');mod=importlib.util.module_from_spec(spec);spec.loader.exec_module(mod)
css={}
for key,left,right,filename in [('main','platform-css-control','platform-css-final','urls.json'),('workers','platform-extra-control','platform-extra-final','supplement.json')]:
 sel=json.loads((g/filename).read_bytes());a,b=mod.read(left),mod.read(right);assert set(a)==set(b)==set(sel)
 sums,details=mod.compare(a,b,sel);css[key]={'summary':sums,'details':details}
(p/'css-summary.json').write_text(json.dumps(css,indent=2))
for k,v in css.items():print(k,v['summary']['gained'],v['summary']['lost'],v['summary']['strict'])
old=json.loads((d/'loss-audit-batch2/canonical-losses.json').read_bytes())
names={(v['url'],v['canonical'],v['occurrence']) for v in old}
new={}
with gzip.open(p/'historical-names.jsonl.gz','rt') as f:
 for line in f:
  u,n,i,*cols=json.loads(line)
  if (u,n,i) in names:new[u,n,i]=[x[0] if x else None for x in cols]
hist=[[v['url'],v['canonical'],v['occurrence'],*new.get((v['url'],v['canonical'],v['occurrence']),[None,None,None])] for v in old]
(p/'historical-292.json').write_text(json.dumps(hist,indent=1))
for col in (3,4,5):print('historical status',col,collections.Counter(r[col] for r in hist))
res={}
for eng in ('control','final'):
 runs=[]
 for order in (1,2,3):
  rows=json.loads((d/'reconciled'/f'platform-referrer-{eng}-{order}.json').read_bytes())['results']
  runs.append({(r['url'],s['name']):s['status'] for r in rows for s in r['subtests']})
 keys=set.union(*(set(x) for x in runs))
 res[eng]={'passCounts':[sum(s=='Pass' for s in t.values()) for t in runs],'varyingNames':sum(len({x.get(k) for x in runs})>1 for k in keys),'names':[ [*k,*[x.get(k) for x in runs]] for k in sorted(keys)]}
(p/'referrer-repeat-summary.json').write_text(json.dumps(res,indent=1))
