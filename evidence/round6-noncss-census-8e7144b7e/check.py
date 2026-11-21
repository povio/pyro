import collections,gzip,json,sys
from pathlib import Path
p=Path(sys.argv[1])
def rows(n):
 with gzip.open(p/n,'rt') as f:yield from map(json.loads,f)
st=json.loads((p/'summary.json').read_bytes());counts=collections.Counter();observed={};hist=json.loads((p/'historical-292.json').read_bytes());selected={(x['url'],x['canonical'],x['occurrence']):x for x in hist}
for u,h,c,hs,cs,reason in rows('pages.jsonl.gz'):
 assert u not in observed
 observed[u]=[h,c,[[0,0],[0,0]]]
 if c:counts['referenceObserved']+=1
 if not h:continue
 assert c and hs and cs
 counts.update(matched=1,lpFull=int(h[2]),chromeFull=int(c[2]),lpOnlyFull=int(h[2] and not c[2]),chromeOnlyFull=int(c[2] and not h[2]))
assert len(observed)==st['official']==31827
last=None
for u,n,i,a,b in rows('named.jsonl.gz'):
 h,c,sizes=observed[u]
 assert i==(last[2]+1 if last and last[:2]==(u,n) else 1)
 last=u,n,i
 if last in selected:assert selected[last]['combined']==a and selected[last]['combinedPage']==h
 for col,x in enumerate([a,b]):
  if x:sizes[col][1]+=1;sizes[col][0]+=int(x[0]=='Pass')
 if h:
  ap=int(a is not None and a[0]=='Pass');bp=int(b is not None and b[0]=='Pass');counts.update(lpPass=ap,chromePass=bp,lpOnly=int(ap and not bp),chromeOnly=int(bp and not ap))
for u,(h,c,sizes) in observed.items():
 for row,size in zip([h,c],sizes):
  if row:assert row[3:5]==size and row[2]==bool(size[1] and size[0]==size[1] and row[:2]==['complete','OK'])
assert counts==st['counts'] and st['missingEligible']==0 and counts['matched']==30358 and len(hist)==len(selected)==292
print('readback OK',dict(counts))
