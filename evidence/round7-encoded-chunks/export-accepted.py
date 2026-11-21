import collections,gzip,hashlib,json,shutil
from pathlib import Path
p=Path(__file__).resolve().parent;r=p.parents[1]/'round6-reference-158/census'
out=p/'evidence';out.mkdir(exist_ok=True)
load=lambda f:json.loads(f.read_bytes())
urls={u:k for k in ['selection','harness','adjacent'] for u in load(p/(k+'.json'))}
assert len(urls)==90
by={};meta={};stats=collections.defaultdict(collections.Counter)
def ingest(f,label):
 raw=load(f); meta[label+'/'+f.name]={'sha256':hashlib.sha256(f.read_bytes()).hexdigest(),'jobs':raw['jobs'],'restarts':raw['restarts'],'watchdog':raw['watchdogKills']};assert raw['wptRevision']=='a421f2fea5eb7f91e0db399e44b0fb0779ee1603' and raw['manifestInventory'] and raw['http2'] and raw['testdriver']
 for row in raw['results']:
  u=row['url']
  if u not in urls:continue
  ct=collections.Counter();items={}
  for z in row['subtests']:
   name=z['name'];ct[name]+=1;items[name,ct[name]]=z['status']
  passes=sum(v=='Pass' for v in items.values())
  full=bool(items) and passes==len(items) and row['status']=='complete' and row['harnessStatus']=='OK'
  assert (u,label) not in by
  by[u,label]=([row['status'],row['harnessStatus'],full,passes,len(items)],items)
for label in ['base','accepted']:
 for suffix in ['', '-harness', '-adjacent']:ingest(p/(label+suffix+'.json'),label)
for f in (r/'selections').glob('*.json'):
 if urls.keys()&set(load(f)):ingest(r/'raw'/f.name,'chrome')
with gzip.open(out/'named.jsonl.gz','wt') as nn,gzip.open(out/'pages.jsonl.gz','wt') as pp:
 for u,group in sorted(urls.items()):
  dat=[by[u,label] for label in ['base','accepted','chrome']];ct=stats[group]
  print(json.dumps([u,group,*[x[0] for x in dat]]),file=pp)
  for label,(pg,_) in zip(['base','accepted','chrome'],dat):ct[label+'/full']+=pg[2];ct[label+'/passes']+=pg[3];ct[label+'/'+pg[0]]+=1
  for n,i in sorted(set.union(*(set(x[1]) for x in dat))):
   a,b,c=[x[1].get((n,i)) for x in dat]
   print(json.dumps([u,n,i,a,b,c]),file=nn)
   ct['gains']+=a!='Pass' and b=='Pass';ct['losses']+=a=='Pass' and b!='Pass';ct['matchedChromeGains']+=a!='Pass' and b==c=='Pass';ct['notRunToFail']+=a=='Not Run' and b=='Fail';ct['namesChanged']+=(a is None)!=(b is None)
(out/'summary.json').write_text(json.dumps(stats,indent=2)+'\n');(out/'receipts.json').write_text(json.dumps(meta,indent=2)+'\n')
print(dict(stats))
