"""Verify occurrence-key normalization and all five pairwise pass totals offline."""
import collections,gzip,json,sys
from pathlib import Path
from wpt_ports import canonical_name,DEFAULTS
root=Path(sys.argv[1]) if len(sys.argv)>1 else Path(__file__).resolve().parent
def rows(name):
 with gzip.open(root/name,'rt') as f:
  for line in f:yield json.loads(line)
configs={}
with gzip.open(root/'port-configs.json.gz','rt') as f:
 for sha,c in json.load(f).items():
  m={}
  assert c['browserHost']=='web-platform.test' and c['serverHost']=='127.0.0.1'
  for role,ports in c['ports'].items():
   if role not in DEFAULTS:continue
   for actual,standard in zip(ports,DEFAULTS[role]):
    scheme='https' if role=='h2' else role.split('-')[0]
    m[scheme,actual]=standard
  configs[sha]=m
pages={u:r for u,env,*r in rows('pages.jsonl.gz')}
maps={u:[configs[x] for x in refs] for u,*refs in rows('port-provenance.jsonl.gz')}
assert pages.keys()==maps.keys()
labels=['combined2','chromeWpt','publicStable','publicCanary']
totals=collections.defaultdict(collections.Counter)
previous=None
for u,name,i,*cols in rows('named.jsonl.gz'):
 key=(u,name,i)
 assert key!=previous
 previous=key
 for j,col in enumerate(cols[:3]):
  if col is not None:assert canonical_name(col[1],maps[u][j])==name
 h=cols[0] is not None and cols[0][0]=='Pass'
 for j,label in enumerate(labels,1):
  if pages[u][j] is None:continue
  r=cols[j] is not None and cols[j][0].upper()=='PASS'
  totals[label]['both' if h and r else 'onlyHead' if h else 'onlyRef' if r else 'neither']+=1
summ=json.loads((root/'summary.json').read_bytes())
for k,c in totals.items():
 for name in ['both','onlyHead','onlyRef']:assert c[name]==summ['comparisons'][k][name],(k,name,c[name],summ['comparisons'][k][name])
print('Verified',len(pages),'pages and',len(configs),'configurations from the distribution alone.')
