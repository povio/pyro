"""Re-read all exported page and subtest observations."""
from pathlib import Path
import collections,gzip,json,sys
p=Path(sys.argv[1])
pages=json.loads((p/'pages.json').read_bytes());sel=json.loads((p/'selection.json').read_bytes())
assert set(sel)==set(pages) and len(pages)==85
labels=('before','single','after','chrome')
sets={c:set() for c in labels}; counts=collections.Counter()
prev=None
with gzip.open(p/'names.jsonl.gz','rt') as f:
 for line in f:
  u,n,i,*v=json.loads(line); key=(u,n,i)
  assert prev is None or prev<key
  assert i==(prev[2]+1 if prev and prev[:2]==key[:2] else 1);prev=key
  assert len(v)==4 and u in pages
  for c,r in zip(labels,v):
   if r:
    counts[c,u]+=1
    if r[0]=='Pass':sets[c].add(key)
for u,rows in pages.items():
 for c,r in rows.items():
  assert r[4]==counts[c,u]
  assert r[3]==sum(k[0]==u for k in sets[c])
  assert r[2]==bool(r[0]=='complete' and r[1]=='OK' and r[4]>0 and r[3]==r[4])
s=json.loads((p/'summary.json').read_bytes())
assert s['gained']==len(sets['after']-sets['before']) and s['lost']==len(sets['before']-sets['after'])
assert s['singleToStackGain']==len(sets['after']-sets['single']) and s['singleToStackLoss']==len(sets['single']-sets['after'])
assert s['lpOnly']==len(sets['after']-sets['chrome']) and s['chromeOnly']==len(sets['chrome']-sets['after'])
print(s)
