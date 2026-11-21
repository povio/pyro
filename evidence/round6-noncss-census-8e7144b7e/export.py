"""Stream immutable Chrome and exact-head native receipts; preserve raw and normalized names."""
from pathlib import Path
from functools import lru_cache
import collections,gzip,json,sys
from plan import p,ref,guard,cache,load,sha,save
sys.path.insert(0,str(ref))
from port_receipt import snapshot_config
from wpt_ports import index
out=Path(sys.argv[1]);out.mkdir(exist_ok=True)
ident=load(ref/'reference-identity.json')
binary=guard/'candidate';head=(guard/'commit.txt').read_text().strip()
assert head=='8e7144b7e7de68be3b5b345138abe2c1dcc6d232' and sha(binary)==(guard/'candidate.sha256').read_text().split()[0]
official={r['url']:r for l in gzip.open(ref/'plans/official-inventory.jsonl.gz','rt') if (r:=json.loads(l))['scope']=='non-CSS'}
holds=load(p/'new-holds.json');reuse=load(p/'guard-membership.json')
new={u:t['key'] for t in load(p/'tasks.json') for u in load(p/'selections'/(t['key']+'.json'))}
assert not reuse.keys()&new.keys(); assert len(new)+len(reuse)+len(holds)==30359
prior=load(ref/'export-final/historical-cbdeb-292.json')
hist={(r['url'],r['canonical'],r['occurrence']):r for r in prior}
ledger={lab:{r['key']:r for l in f.read_text().splitlines() if (r:=json.loads(l))} if f.exists() else {} for lab,f in [('remaining',p/'raw/completed.jsonl'),('guard',guard/'raw-head/completed.jsonl')]}
ledger['chrome']=load(ref/'export-final/receipt-provenance.json')
parts={}
@lru_cache(maxsize=4)
def receipt(lab,k):
 raw={'chrome':ref/'raw','guard':guard/'raw-head','remaining':p/'raw'}[lab]
 server={'chrome':ref/'verified-ports/chrome','guard':p/'guard-ports','remaining':p/'verified-ports'}[lab]
 path=raw/(k+'.json');digest=sha(path);r=ledger[lab][k]
 assert digest==r.get('rawSHA256',r.get('receiptSHA256'))
 log=raw/(k+'.log');m,proof=snapshot_config(log,server/(k+'.json'))
 raw=load(path);assert raw['wptRevision']==ident['wpt'] and raw['inventoryPlan']['manifestSha256']==ident['manifestSha256']
 assert raw['jobs']==4 and raw['testdriver'] and raw['http2'] and raw['server']=='wptserve' and not raw['reuseContext']
 assert raw['inventoryPlan']['viewport']==dict(width=800,height=600,deviceScaleFactor=1)
 parts[lab+'/'+k]=dict(receipt=digest,log=sha(log),ports=proof['ports'],config=proof['sha256'],restarts=raw['restarts'],watchdogKills=raw['watchdogKills'])
 return {r['url']:r for r in raw['results']},m
def view(pair,u):
 if not pair:return None,{}
 rows,m=pair;r=rows[u];assert all(r[key]==official[u][key] for key in ['environment','blob','source'])
 a,clash=index([(t['name'],t['status']) for t in r['subtests']],m);assert not clash
 passed=sum(t[0]=='Pass' for t in a.values())
 return [r['status'],r['harnessStatus'],bool(a) and len(a)==passed and r['status']=='complete' and r['harnessStatus']=='OK',passed,len(a)],a
def emit(f,x):print(json.dumps(x,ensure_ascii=True,separators=(',',':')),file=f)
ct=collections.Counter();seen=set()
with gzip.open(out/'pages.jsonl.gz','wt',compresslevel=3) as pg,gzip.open(out/'named.jsonl.gz','wt',compresslevel=3) as nm:
 for t in load(ref/'plans/tasks.json'):
  k=t['key'];urls=[u for u in load(ref/'selections'/(k+'.json')) if u in official]
  if not urls:continue
  chrome=receipt('chrome',k)
  for u in urls:
   assert u not in seen;seen.add(u)
   lab='guard' if u in reuse else 'remaining';key=reuse.get(u,new.get(u))
   candidate=receipt(lab,key) if key in ledger[lab] else None
   a,an=view(candidate,u);b,bn=view(chrome,u)
   emit(pg,[u,a,b,lab+'/'+key if candidate else None,'chrome/'+k,holds.get(u)]);ct['referenceObserved']+=1
   if a:
    ct['matched']+=1;ct['lpFull']+=a[2];ct['chromeFull']+=b[2]
    ct['lpOnlyFull']+=a[2] and not b[2];ct['chromeOnlyFull']+=b[2] and not a[2]
   for n,i in sorted(an.keys()|bn.keys()):
    x=an.get((n,i));y=bn.get((n,i));emit(nm,[u,n,i,x,y])
    if (u,n,i) in hist:hist[u,n,i]['combined']=x;hist[u,n,i]['combinedPage']=a
    if a:
     xp=bool(x and x[0]=='Pass');yp=bool(y and y[0]=='Pass')
     ct['lpPass']+=xp;ct['chromePass']+=yp;ct['lpOnly']+=xp and not yp;ct['chromeOnly']+=yp and not xp
   for row in prior:
    if row['url']==u:row.setdefault('combinedPage',a);row.setdefault('combined',None)
 for u in sorted(official.keys()-seen):emit(pg,[u,None,None,None,None,official[u]['withheld']])
for row in prior:row.setdefault('combinedPage',None);row.setdefault('combined',None)
assert len(seen)==30359 and len(official)==31827 and len(hist)==292
save(out/'summary.json',dict(head=head,binarySHA256=sha(binary),wpt=ident['wpt'],chrome=ident['version'],official=len(official),historicalWithheld=len(official)-len(seen),newHolds=holds,missingEligible=len(new)+len(reuse)-ct['matched'],counts=ct))
save(out/'historical-292.json',prior);save(out/'provenance.json',parts)
print(dict(ct))
