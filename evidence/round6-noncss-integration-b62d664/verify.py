"""Recompute observed named and page sets for every cohort and comparator."""
from collections import Counter,defaultdict
from pathlib import Path
import gzip,json,sys
p=Path(sys.argv[1]);cols=json.loads((p/'columns.json').read_bytes())
def rows(n):
 with gzip.open(p/n,'rt') as f:yield from map(json.loads,f)
selected=json.loads((p/'selected.json').read_bytes());stats=defaultdict(dict);pairs=defaultdict(dict);pages={};observed=defaultdict(Counter);delta=[]
order=[(0,1),(2,1),(3,1),(4,1),(5,1)]
for u,co,o,*ps in rows('pages.jsonl.gz'):
 assert co==selected[u] and u not in pages;pages[u]=ps
 assert len(ps)==len(cols) and ps[0] and ps[1]
 for j,r in enumerate(ps):
  s=stats[co].setdefault(cols[j],Counter())
  if r is None:s['missingPages']+=1;continue
  assert r[2]==bool(r[0]=='complete' and r[1] in (0,'OK') and r[3]==r[4] and r[4])
  s.update(pages=1,fullPages=int(r[2]),namedPass=r[3],namedTotal=r[4])
  s['page/'+r[0]]+=1
 for l,h in order:
  s=pairs[co].setdefault(cols[l]+' to '+cols[h],Counter())
  if ps[l] and ps[h]:
   s['pairedPages']+=1;s['gainedPages']+=ps[h][2] and not ps[l][2];s['lostPages']+=ps[l][2] and not ps[h][2]
assert set(pages)==set(selected)
last=None
for u,n,i,*vs in rows('names.jsonl.gz'):
 key=u,n,i;assert last is None or last<key;assert i==(last[2]+1 if last and (u,n)==last[:2] else 1);last=key
 co=selected[u]
 for j,v in enumerate(vs):
  if v:assert pages[u][j];observed[u,j]['pass']+=v[0]=='Pass';observed[u,j]['total']+=1
 for l,h in order:
  if not pages[u][l] or not pages[u][h]:continue
  s=pairs[co][cols[l]+' to '+cols[h]]
  lp=bool(vs[l] and vs[l][0]=='Pass');hp=bool(vs[h] and vs[h][0]=='Pass')
  s['commonPass']+=lp and hp;s['gainedPass']+=hp and not lp;s['lostPass']+=lp and not hp
  if l==0 and lp!=hp:delta.append([u,n,i,*vs])
for u,ps in pages.items():
 for j,page in enumerate(ps):
  if page:assert [observed[u,j]['pass'],observed[u,j]['total']]==page[3:],(u,j,observed[u,j],page)
val={'browsers':stats,'comparisons':pairs};dest=p/'summary.json'
if '--write' in sys.argv:
 dest.write_text(json.dumps(val,indent=1)+'\n')
 with gzip.open(p/'base-to-head-changes.jsonl.gz','wt') as f:
  for r in delta:print(json.dumps(r,separators=(',',':')),file=f)
else:assert val==json.loads(dest.read_bytes()) and delta==list(rows('base-to-head-changes.jsonl.gz'))
print('Verified',len(pages),'unique observed LP URLs and every named occurrence for all available columns.')
