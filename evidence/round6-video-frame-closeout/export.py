"""Rebuild directional page/name evidence from matched LP and frozen Chrome receipts."""
import collections, gzip, hashlib, json, os, shutil, sys
from pathlib import Path
n=Path(__file__).resolve().parent
cache=Path(os.environ.get('LP_CACHE',Path.home()/'Library/Caches/lightningpanda'))
reference=cache/'round6-reference-158/census'
sys.path.insert(0,str(cache/'round6-noncss-measure/manifest-runs/census-902fe811f'))
from wpt_ports import index,config
out=n/'evidence'
out.mkdir(exist_ok=True)
read=lambda p:json.loads(p.read_bytes())
digest=lambda p:hashlib.sha256(p.read_bytes()).hexdigest()
def write(name,obj): (out/name).write_text(json.dumps(obj,indent=2)+'\n')
def emit(file,row): print(json.dumps(row,ensure_ascii=True,separators=(',',':')),file=file)
urls={u:key for key in ('focus','guard') for u in read(n/(key+'.json'))}
identity=read(reference/'reference-identity.json')
pages,sets,provenance={},{},{}
def add(path,label,key,owners):
 report=read(path);log=path.with_suffix('.log')
 assert report['server']=='wptserve' and report['testdriver'] and report['http2'] and not report['reuseContext']
 assert report['jobs']==4 and report['inventoryPlan']['viewport']==dict(zip(('width','height','deviceScaleFactor'), identity['viewport']))
 assert report['inventoryPlan']['manifestSha256']==identity['manifestSha256']
 assert report['wptRevision']==identity['wpt'] and set(report['plan'])=={r['url'] for r in report['results']}
 if label!='chrome':
  assert set(report['plan'])==owners and not report['restarts'] and not report['watchdogKills']
 mp,info=config(str(log));assert 'error' not in info
 provenance[label+'/'+key]={'receipt':digest(path),'serverConfig':info['sha256'],'ports':info['ports'],
  'log':digest(log),'observedInShard':len(report['results']),'totalShardRestarts':report['restarts'],'totalShardWatchdogKills':report['watchdogKills']}
 for row in report['results']:
  u=row['url']
  if u not in owners:continue
  id=(u,label);assert id not in sets
  names,collisions=index([(t['name'],t['status']) for t in row['subtests']],mp);assert not collisions
  sets[id]=names
  passed=sum(v[0]=='Pass' for v in names.values())
  pages[id]=[row['status'],row['harnessStatus'],bool(names) and passed==len(names) and row['harnessStatus']=='OK' and row['status']=='complete',passed,len(names),key]
for k in ('focus','guard'):
 for lab in ('base','vetted'):add(n/f'{lab}-{k}.json',lab,k,{u for u,co in urls.items() if co==k})
for p in (reference/'selections').glob('*.json'):
 owners=set(read(p))&urls.keys()
 if owners:add(reference/'raw'/p.name,'chrome',p.stem,owners)
counts=collections.defaultdict(collections.Counter)
labs=['base','vetted','chrome']
with gzip.open(out/'pages.jsonl.gz','wt') as pp,gzip.open(out/'names.jsonl.gz','wt') as nn:
 for u,k in sorted(urls.items()):
  c=counts[k];c['urls']+=1
  pgs=[pages[u,l] for l in labs]
  emit(pp,[u,k,*pgs])
  for l,pg in zip(labs,pgs):
   c[l+'FullPages']+=pg[2];c[l+'Passes']+=pg[3]
  for name,occ in sorted(set().union(*(sets[u,l].keys() for l in labs))):
   obs=[sets[u,l].get((name,occ)) for l in labs]
   emit(nn,[u,name,occ,*obs])
   a,b,h=[v is not None and v[0]=='Pass' for v in obs]
   c['gains']+=b and not a;c['losses']+=a and not b
   c['gainsChromePass']+=b and not a and h;c['candidateOnlyVsChrome']+=b and not h;c['chromeOnlyVsCandidate']+=h and not b
write('summary.json',counts);write('receipts.json',provenance)
shutil.copyfile(reference/'reference-identity.json',out/'chrome-identity.json')
for f in ('focus.json','guard.json','verify-pixels.mjs','run.sh','export.py'):shutil.copyfile(n/f,out/f)
print(json.dumps(counts,indent=2))
