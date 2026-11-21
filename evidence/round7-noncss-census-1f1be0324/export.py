"""Preserve the exact four-job candidate, exact frozen Chrome and prior census as distinct columns."""
from common import *
import collections,contextlib,functools,gzip,itertools,sys
sys.path.insert(0,str(ref))
from port_receipt import snapshot_config
from wpt_ports import index
def rows(f):
 with gzip.open(f,'rt') as s:yield from map(json.loads,s)
def emit(s,x):s.write(json.dumps(x,separators=(',',':'),ensure_ascii=True)+'\n')
def named(r,m):
 a,collision=index([(t['name'],t['status']) for t in r['subtests']],m);assert not collision
 passes=sum(x[0]=='Pass' for x in a.values())
 pg=[r['status'],r['harnessStatus'],bool(a) and len(a)==passes and r['status']=='complete' and r['harnessStatus']=='OK',passes,len(a)]
 return pg,a
def diff(c,a,b):
 ap=a is not None and a[0]=='Pass';bp=b is not None and b[0]=='Pass'
 c.update(firstPass=int(ap),secondPass=int(bp),commonPass=int(ap and bp),firstOnly=int(ap and not bp),secondOnly=int(bp and not ap),missingFirstName=int(a is None and b is not None),missingSecondName=int(b is None and a is not None))
def page_diff(c,a,b):
 c.update(matched=1,firstFull=int(a[2]),secondFull=int(b[2]),firstOnlyFull=int(a[2] and not b[2]),secondOnlyFull=int(b[2] and not a[2]))
def readback(out):
 out.mkdir(exist_ok=True)
 tasks={t['key']:t for t in load(p/'tasks.json')};led=p/'raw/completed.jsonl'
 ledger={r['key']:r for l in led.read_text().splitlines() if (r:=json.loads(l))} if led.exists() else {}
 assert len(ledger)==len(led.read_text().splitlines()) if led.exists() else not ledger
 new={}
 for k,t in tasks.items():
  f=p/'selections'/(k+'.json');assert sha(f)==t['sha256']
  for u in load(f):assert u not in new;new[u]=k
 prior=old/'evidence';before={};meta={};events={}
 for l in (prior/'SHA256SUMS').read_text().splitlines():
  checksum,n=l.split('  ',1);before[n]=checksum
 for n in ['pages.jsonl.gz','named.jsonl.gz','summary.json','historical-292.json','provenance.json']:assert sha(prior/n)==before[n]
 assert load(prior/'summary.json')['head']=='8e7144b7e7de68be3b5b345138abe2c1dcc6d232'
 identity=load(ref/'reference-identity.json')
 def meta_for(k):
  f=p/'raw'/(k+'.json');assert sha(f)==ledger[k]['receiptSHA256']
  raw=load(f);log=f.with_suffix('.log');m,proof=snapshot_config(log,p/'verified-ports'/(k+'.json'))
  assert raw['wptRevision']==identity['wpt'] and raw['inventoryPlan']['manifestSha256']==identity['manifestSha256'] and raw['bin']==str(binary)
  assert raw['jobs']==4 and raw['http2'] and raw['testdriver'] and raw['server']=='wptserve' and not raw['reuseContext']
  assert len(raw['results'])==tasks[k]['count'] and set(raw['plan'])=={r['url'] for r in raw['results']}==set(load(p/'selections'/(k+'.json')))
  meta[k]=dict(receiptSHA256=ledger[k]['receiptSHA256'],logSHA256=sha(log),selectionSHA256=tasks[k]['sha256'],ports=proof['ports'],configSHA256=proof['sha256'],restarts=raw['restarts'],watchdogKills=raw['watchdogKills'])
  bad=[{'url':r['url'],'status':r['status']} for r in raw['results'] if r['status'] in ('hang','crash','panic')]
  if bad or raw['restarts'] or raw['watchdogKills']:
   ambiguous=raw['watchdogKills']>0 or raw['restarts']>len(bad)
   events[k]=dict(observed=bad,watchdogKills=raw['watchdogKills'],restarts=raw['restarts'],unassignedNativeExitPossible=ambiguous,withholdFromFutureRuns=raw['plan'] if ambiguous else [v['url'] for v in bad],retried=False)
  return {r['url']:r for r in raw['results']},m
 receipt=functools.lru_cache(maxsize=2)(meta_for)
 previous=itertools.groupby(rows(prior/'named.jsonl.gz'),key=lambda r:r[0]);group=next(previous,None)
 counts=collections.defaultdict(collections.Counter);timing=collections.Counter();seen=set();matches=0;held=load(p/'holds.json');historic=load(prior/'historical-292.json');hist={(r['url'],r['canonical'],r['occurrence']):r for r in historic}
 archive_urls={r['url'] for n in load(p/'frozen-archives.json') for r in load(cache/n)['results']}
 archive_urls.update(load(p/'smoke-overlaps.json'))
 picked={};aux=out/'candidate-profile-overlap.jsonl.gz'
 with gzip.open(out/'pages.jsonl.gz','wt',compresslevel=3) as pg,gzip.open(out/'named.jsonl.gz','wt',compresslevel=3) as ns,gzip.open(aux,'wt',compresslevel=3) as sel:
  for u,a,b,old_key,chrome_key,reason in rows(prior/'pages.jsonl.gz'):
   assert u not in seen;seen.add(u)
   k=new.get(u);row=None;names={}
   if k in ledger:
    raw,m=receipt(k);r=raw[u];row,names=named(r,m);matches+=1
   if group is not None and group[0]==u:
    original={(n,i):(v,w) for _,n,i,v,w in group[1]};group=next(previous,None)
   else:original={}
   pp=[row,b,a];keys=[k if row else None,chrome_key,old_key]
   emit(pg,[u,*pp,keys,held.get(u,reason)])
   if row:
    assert b and a
    page_diff(counts['headChrome'],row,b);page_diff(counts['headPrevious'],row,a);timing[row[0]]+=1
   if row and u in archive_urls:emit(sel,[u,row,[[n,i,*v] for (n,i),v in names.items()]])
   for n,i in sorted(names.keys()|original.keys()):
    h=names.get((n,i));a,b=original.get((n,i),(None,None))
    emit(ns,[u,n,i,h,b,a])
    if row:
     if h is not None or b is not None:diff(counts['headChrome'],h,b)
     if h is not None or a is not None:diff(counts['headPrevious'],h,a)
    if (u,n,i) in hist:hist[u,n,i]['round7']=h
   for it in historic:
    if it['url']==u:it['round7Page']=row;it.setdefault('round7',None)
 assert group is None and len(seen)==31827 and matches==sum(tasks[k]['count'] for k in ledger)
 for r in historic:r.setdefault('round7',None);r.setdefault('round7Page',None)
 save(out/'summary.json',dict(head=head,binarySHA256=sha(binary),chrome=identity['version'],wpt=identity['wpt'],manifestSHA256=identity['manifestSha256'],priorHead=load(prior/'summary.json')['head'],manifestCount=len(seen),frozenEligible=30359,newHolds=held,scheduled=len(new),matched=matches,missingEligible=len(new)-matches,lifecycle=timing,comparisons=counts,completeShards=len(ledger),totalShards=len(tasks)))
 save(out/'observed-native-events.json',events);save(out/'raw-provenance.json',meta);save(out/'historical-292.json',historic)
if __name__=='__main__':readback(Path(sys.argv[1]))
