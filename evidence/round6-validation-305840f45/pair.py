"""Compare combined binary observations against each other on the matching frozen selections."""
from pathlib import Path
import json,collections,gzip
n=Path(__file__).resolve().parent
p=n.parent/'combined-db594cd99'
def named(f):
 return {(u,s,i):b for u,s,i,_,b in map(json.loads,gzip.open(f,'rt')) if b}
groups={}
with gzip.open(n/'evidence/db594-to-final-changes.jsonl.gz','wt') as out:
 for f in sorted(p.glob('*.named.jsonl.gz')):
  k=f.name.split('.')[0]
  a=named(f);b=named(n/f.name);c=collections.Counter()
  for name in sorted(a.keys() | b.keys()):
   va=a.get(name);vb=b.get(name);aa=bool(va and va[0]=='Pass');bb=bool(vb and vb[0]=='Pass')
   if aa!=bb:
    c['gains' if bb else 'losses']+=1;print(json.dumps([k,*name,va,vb]),file=out)
  pf=k+'.pages.json'
  def pages(p):return {u:full for u,_,_,_,full in json.loads(p.read_bytes())}
  ap=pages(p/pf);bp=pages(n/pf);assert ap.keys()==bp.keys()
  c['urls']=len(ap);c['fullGains']=sum(bp[u] and not ap[u] for u in ap);c['fullLosses']=sum(ap[u] and not bp[u] for u in ap)
  groups[k]=dict(c)
(n/'evidence/db594-to-final.json').write_text(json.dumps(groups,indent=1)+'\n')
print(groups)
