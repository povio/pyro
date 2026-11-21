"""Group observed gains/losses by WPT top-level directory; retain every lost pass."""
from pathlib import Path
import collections,gzip,json,sys
p=Path(sys.argv[1])
def rows(n):
 with gzip.open(p/n,'rt') as f:
  for line in f:yield json.loads(line)
pages={u:(k,co,scope,b,h,c) for u,k,co,scope,d,b,h,c in rows('pages.jsonl.gz')}
ct=collections.defaultdict(collections.Counter)
sizes=collections.Counter()
transition=collections.defaultdict(lambda:collections.defaultdict(collections.Counter))
with gzip.open(p/'lost-names.jsonl.gz','wt') as f:
 for u,n,i,b,h,c in rows('named.jsonl.gz'):
  k,co,scope,bp,hp,cp=pages[u]
  if not bp or not hp:continue
  a=b[0] if b is not None else 'missing';z=h[0] if h is not None else 'missing'
  if a!=z:transition[co+'/'+scope]['namedStatusChanges'][a+' -> '+z]+=1
  area=u.split('/')[0];cs=ct[co+'/'+area]
  vb=b is not None and b[0]=='Pass';vh=h is not None and h[0]=='Pass'
  if vh and not vb:cs['gains']+=1
  if vb and not vh:
   cs['losses']+=1
   cls='chrome-unobserved' if not cp else 'chrome-missing-name' if c is None else 'chrome-'+c[0]
   sizes[cls]+=1
   print(json.dumps([u,n,i,b,h,c,bp,hp,cp],separators=(',',':')),file=f)
for u,(k,co,scope,b,h,c) in pages.items():
 if b and h:
  pg=lambda v:v[0]+'/'+str(v[1])
  if pg(b)!=pg(h):transition[co+'/'+scope]['pageStatusChanges'][pg(b)+' -> '+pg(h)]+=1
  cs=ct[co+'/'+u.split('/')[0]];cs['pages']+=1;cs['fullPageGains']+=h[2] and not b[2];cs['fullPageLosses']+=b[2] and not h[2]
(p/'effects.json').write_text(json.dumps(dict(areas=ct,lostPassChromeStatus=sizes,transitions=transition),indent=1)+'\n')
