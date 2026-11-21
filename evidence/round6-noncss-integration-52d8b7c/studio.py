"""Keep a second independent signed stable comparison for the already paired CSS controls."""
import collections,gzip,hashlib,json,sys
from pathlib import Path
p=Path(sys.argv[1]);ref=Path(sys.argv[2]);stats=collections.Counter()
def rows(d,n):
 with gzip.open(d/n,'rt') as f:
  for line in f:yield json.loads(line)
def put(f,r):f.write(json.dumps(r,separators=(',',':'))+'\n')
pages={u:(dims,h,c) for u,k,co,scope,dims,b,h,c in rows(p,'pages.jsonl.gz') if scope=='css'};assert len(pages)==1131
observed=set()
with gzip.open(p/'studio-css-candidate-pages.jsonl.gz','wt') as pf:
 for u,source,blob,env,cohort,stud,local,k,_,__ in rows(ref,'studio-css-pages.jsonl.gz'):
  if u not in pages:continue
  dims,h,c=pages[u];assert (c[:3]+[c[4],c[3]] if c else None)==local and (not dims or dims[:3]==[source,blob,env]) and stud
  if h:
   observed.add(u);stats['paired']+=1;stats['headFull']+=h[2];stats['studioFull']+=stud[2]
   stats['headOnlyFull']+=h[2] and not stud[2];stats['studioOnlyFull']+=stud[2] and not h[2]
  put(pf,[u,h,stud,cohort,c])
studnames={(u,n,i):(st,local) for u,n,i,st,local in rows(ref,'studio-css-names.jsonl.gz') if u in pages}
with gzip.open(p/'studio-css-candidate-names.jsonl.gz','wt') as nf:
 def write(u,n,i,h,st,lo):
  put(nf,[u,n,i,h,st,lo])
  if u in observed:
   hp=h is not None and h[0]=='Pass';sp=st=='Pass';stats['headPass']+=hp;stats['studioPass']+=sp
   stats['commonPass']+=hp and sp;stats['headOnlyPass']+=hp and not sp;stats['studioOnlyPass']+=sp and not hp
 for u,n,i,b,h,c in rows(p,'named.jsonl.gz'):
  if u not in pages:continue
  st,lo=studnames.pop((u,n,i),(None,None))
  assert lo==(c[0] if c else None),(u,n,i,lo,c)
  if h is not None or st is not None or lo is not None:write(u,n,i,h,st,lo)
 for (u,n,i),(st,lo) in sorted(studnames.items()):
  assert lo is None;write(u,n,i,None,st,lo)
(p/'studio-css-candidate-summary.json').write_text(json.dumps(stats,indent=1)+'\n')
print(dict(stats))
