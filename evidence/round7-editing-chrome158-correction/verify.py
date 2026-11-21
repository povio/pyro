"""Independent readback of corrected page and named status sets from the portable export."""
import collections,gzip,json,sys
from pathlib import Path
p=Path(sys.argv[1]);get=lambda f:json.loads((p/f).read_bytes())
def each(f):
 with gzip.open(p/f,'rt') as z:yield from map(json.loads,z)
spec=get('summary.json');cols=spec['columns'];cs=get('comparisons.json');scope=get('receipt-mapping.json');sel=set(get('urls.json'))
pages={};tables=collections.defaultdict(lambda:[{} for _ in cols]);global_names=collections.defaultdict(dict);global_pages=collections.defaultdict(dict);seen=set()
for group,u,*ps in each('page-sets.jsonl.gz'):
 k=group,u;assert k not in pages and u in sel and group in scope and len(ps)==len(cols);pages[k]=ps
 for i,v in enumerate(ps):
  assert v[2]==bool(v[0]=='complete' and v[1] in (0,'OK') and v[3]==v[4] and v[4])
  if i>=2:
   if u in global_pages[i]:assert global_pages[i][u]==v
   global_pages[i][u]=v
for group,u,name,occ,*vs in each('named-sets.jsonl.gz'):
 k=(group,u,name,occ);assert k not in seen;seen.add(k);assert (group,u) in pages and occ>0
 for i,v in enumerate(vs):
  if v:
   tables[group,u][i][name,occ]=v
   if i>=2:
    old=global_names[i].setdefault((u,name,occ),v);assert old==v
for (g,u),ps in pages.items():
 for i,pg in enumerate(ps):
  t=tables[g,u][i];assert len(t)==pg[4] and sum(v[0]=='Pass' for v in t.values())==pg[3]
assert global_names[2]==global_names[3] and global_pages[2]==global_pages[3]
assert set(global_pages[2])==set(global_pages[3])==set(global_pages[4])==sel and len(sel)==47
for i,lab in enumerate(cols[2:],2):
 t=global_names[i];ps=global_pages[i];assert dict(pages=len(ps),fullPages=sum(v[2] for v in ps.values()),passes=sum(v[0]=='Pass' for v in t.values()),names=len(t))==spec['distinct'][lab]
assert set(cs)==set(scope)
for g,v in cs.items():
 for i,lab in enumerate(cols):
  rows=[ps[i] for (group,u),ps in pages.items() if group==g];r=v['columns'][lab];assert [len(rows),sum(a[2] for a in rows),sum(a[3] for a in rows),sum(a[4] for a in rows)]==[r[x] for x in ['pages','full','passes','names']]
 for a,b in [(0,1),(2,3),(3,4),(1,3),(1,4)]:
  delta=collections.Counter()
  for (group,u),ts in tables.items():
   if group!=g:continue
   aa={key for key,val in ts[a].items() if val[0]=='Pass'};bb={key for key,val in ts[b].items() if val[0]=='Pass'}
   delta.update(gained=len(bb-aa),lost=len(aa-bb),addedNames=len(ts[b].keys()-ts[a].keys()),missingNames=len(ts[a].keys()-ts[b].keys()))
  assert delta==v['comparisons'][cols[a]+'To'+cols[b]]
print(f'Verified {len(sel)} URLs, {len(scope)} receipt mappings, {len(global_names[3])} signed static named occurrences, every page and all five comparison directions.')
