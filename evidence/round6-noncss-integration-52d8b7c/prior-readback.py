"""Compare observed LP statuses across the two integrations on identical selections."""
import collections, gzip, heapq, itertools, json, sys
from pathlib import Path
old,new=map(Path,sys.argv[1:3])
def rows(p,n):
 with gzip.open(p/n,'rt') as f:
  yield from map(json.loads,f)
def save(f,row):
 print(json.dumps(row,separators=(',',':')),file=f)
pages={}
stats=collections.defaultdict(collections.Counter)
with gzip.open(new/'prior-pages.jsonl.gz','wt') as f:
 for a,b in zip(rows(old,'pages.jsonl.gz'),rows(new,'pages.jsonl.gz'),strict=True):
  assert a[:5]==b[:5] and a[5] and a[6] and b[5] and b[6]
  assert a[7] is None or b[7] is not None
  u=a[0];pages[u]=a[2]+'/'+a[3]
  save(f,[u,a[5],b[5],a[6],b[6]])
  for i,label in ((5,'base'),(6,'head')):
   s=stats[pages[u]];s[label+'FullPageGains']+=bool(b[i][2] and not a[i][2]);s[label+'FullPageLosses']+=bool(a[i][2] and not b[i][2])
def named(p,index):
 for row in rows(p,'named.jsonl.gz'):yield (tuple(row[:3]),index,row[3:])
merged=heapq.merge(named(old,0),named(new,1))
with gzip.open(new/'prior-named-changes.jsonl.gz','wt') as f:
 for key,group in itertools.groupby(merged,lambda v:v[0]):
  obs={i:vals for _,i,vals in group}
  a=obs.get(0,[None]*3);b=obs.get(1,[None]*3)
  if a[2] and (b[2] is None or a[2][0]!=b[2][0]):stats[pages[key[0]]]["referenceStatusChanges"]+=1
  # Local server ports may differ in raw assertion names.
  status=lambda x:x[0] if x else None
  if any(status(x)!=status(y) for x,y in zip(a[:2],b[:2])):
   save(f,[*key,a,b])
  for i,label in enumerate(('base','head')):
   was=status(a[i])=='Pass';now=status(b[i])=='Pass'
   s=stats[pages[key[0]]];s[label+'NamedGains']+=now and not was;s[label+'NamedLosses']+=was and not now
(new/'prior-comparison.json').write_text(json.dumps(stats,indent=1)+'\n')
def losses(p):
 return {tuple(r[:3]):r for r in rows(p,'lost-names.jsonl.gz')}
old_losses=losses(old);new_losses=losses(new)
with gzip.open(new/'directional-loss-history.jsonl.gz','wt') as f:
 for key in sorted(old_losses.keys()|new_losses.keys()):
  save(f,[*key,old_losses.get(key),new_losses.get(key)])
def family(k):
 return k[0].split('/')[0]
summary={}
for label,keys in [('previousOnly',old_losses.keys()-new_losses.keys()),('repeated',old_losses.keys()&new_losses.keys()),('currentOnly',new_losses.keys()-old_losses.keys())]:
 summary[label]=dict(total=len(keys),byDirectory=collections.Counter(map(family,keys)))
(new/'directional-loss-summary.json').write_text(json.dumps(summary,indent=1)+'\n')
# Rank observed shared causes by test directory only; no coverage extrapolation.
coverage=collections.defaultdict(collections.Counter);paired=set()
for u,k,co,scope,d,b,h,c in rows(new,'pages.jsonl.gz'):
 ct=coverage[(scope,co,u.split('/')[0])]
 ct['planned']+=1
 if not c:ct['unpairedChrome']+=1;continue
 paired.add(u);ct['paired']+=1
 ct['chromeOnlyFull']+=c[2] and not h[2];ct['headOnlyFull']+=h[2] and not c[2]
for u,name,i,b,h,c in rows(new,'named.jsonl.gz'):
 co,scope=pages[u].split('/')
 ct=coverage[(scope,co,u.split('/')[0])]
 hp=bool(h and h[0]=='Pass');cp=bool(c and c[0]=='Pass')
 if cp and not hp:ct['chromeOnlyName']+=1
 if hp and not cp and u in paired:ct['headOnlyName']+=1
(new/'guard-gaps-by-directory.json').write_text(json.dumps([dict(scope=k[0],cohort=k[1],directory=k[2],**v) for k,v in sorted(coverage.items(),key=lambda a:(-a[1]['chromeOnlyName'],a[0]))],indent=1)+'\n')
