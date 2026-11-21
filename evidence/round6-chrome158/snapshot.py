"""Freeze a completed selection of the 158 WPT receipts and compare only genuinely paired observations."""
from pathlib import Path
import collections,gzip,hashlib,json,os,sqlite3,sys,zlib
d=Path(__file__).resolve().parent;p=d.parents[1];m=p/'round6-noncss-measure/manifest-runs';lp=m/'census-cbdeb1193';base=p/'round6-noncss-measure/merged-aa0268/batch/readback/evidence'
sys.path.insert(0,str(m/'census-902fe811f'));from wpt_ports import canonical_name,index,config,endpoint_map
from port_receipt import snapshot_config
stage=sys.argv[1];out=Path(sys.argv[2]).resolve();out.mkdir(parents=True,exist_ok=True)
assert stage in ('history','priority','final','interim')
G=lambda f:json.loads(f.read_bytes());sha=lambda f:hashlib.sha256(f.read_bytes()).hexdigest();dump=lambda v:json.dumps(v,separators=(',',':'),ensure_ascii=True)
def stream(f):
 with gzip.open(f,'rt') as s:
  for l in s:yield json.loads(l)
def save(name,value):(out/name).write_text(json.dumps(value,indent=2)+'\n')
tasks=G(d/'plans/tasks.json');done={r['key']:r for l in (d/'raw/completed.jsonl').read_text().splitlines() if l.strip() and (r:=json.loads(l))}
if stage=='history':required=[t for t in tasks if t['key'].startswith('guard-history-')]
elif stage=='priority':required=[t for t in tasks if not t['key'].startswith('ordinary')]
elif stage=='interim':required=[t for t in tasks if t['key'] in done]
else:required=tasks
assert all(t['key'] in done for t in required),(stage,'incomplete receipt set')
ident=G(d/'reference-identity.json');assert sha(d/'launch-158')==ident['profileSHA256']
official={r['url']:r for r in stream(d/'plans/official-inventory.jsonl.gz')};eligible={u for u,r in official.items() if r['eligible']}
target={u for t in required for u in G(d/'selections'/(t['key']+'.json'))};assert target<=eligible and len(target)==sum(t['count'] for t in required)
configs={};sources=[];nativeEvents=[];state=collections.defaultdict(collections.Counter)
(obdir:=out/'receipts').mkdir(exist_ok=True)
receipts={};indexByURL={}
for t in required:
 k=t['key'];orig=d/'raw'/(k+'.json');doc=G(orig);urls=set(G(d/'selections'/(k+'.json')))
 assert sha(orig)==done[k]['receiptSHA256'] and len(doc['results'])==len(urls) and {r['url'] for r in doc['results']}==urls
 assert doc['wptRevision']==ident['wpt'] and doc['manifestInventory'] and doc['jobs']==4 and doc['inventoryPlan']['manifestSha256']==ident['manifestSha256'] and doc['inventoryPlan']['viewport']=={'width':800,'height':600,'deviceScaleFactor':1}
 records=list((d/'launch-records'/k).glob('*.json'));assert records
 for f in records:
  launch=G(f);assert launch['mode']=='wpt-stable' and launch['exe']==str(d.parent/'Google Chrome.app/Contents/MacOS/Google Chrome')
 log=d/'raw'/(k+'.log');mapping,proof=snapshot_config(log,d/'verified-ports/chrome'/(k+'.json'));assert 'error' not in proof
 h=proof['sha256'];configs[h]=proof['ports']
 receipts[k]={'rawSHA256':done[k]['receiptSHA256'],'selectionSHA256':t['sha256'],'logSHA256':sha(log),'configurationSHA256':h,'jobs':4,'restarts':doc['restarts'],'watchdogKills':doc['watchdogKills'],'launchCount':len(records)}
 if doc['restarts'] or doc['watchdogKills']:
  marked=[{'url':r['url'],'status':r['status'],'error':r.get('error')} for r in doc['results'] if r['status'] in ('crash','hang','panic') or r.get('error')=='Browser connection lost; observation withheld without replay.']
  hangCount=sum(r['status']=='hang' for r in doc['results'])
  nativeEvents.append({'key':k,'receiptSHA256':done[k]['receiptSHA256'],'restarts':doc['restarts'],'watchdogKills':doc['watchdogKills'],'explicitRows':marked,'unassignedNativeExitPossible':doc['restarts']!=hangCount or bool(doc['watchdogKills']),'rawNamesPreserved':True,'retried':False})
 # Keep every raw subtest/status/error and all runner options. Replace private installation path with a portable executable identity.
 doc['bin']='official-google-chrome-stable-158.0.8127.37';doc['runnerProvenance']=dict(driver='playwright-core 1.65.1',runnerSHA256=ident['noNativeReplayRunnerSHA256'],profileSHA256=ident['profileSHA256'],browserSHA256=ident['executableSHA256'],frameworkSHA256=ident['frameworkSHA256'],rawOriginalSHA256=done[k]['receiptSHA256'])
 with (obdir/(k+'.json.gz')).open('wb') as buf,gzip.GzipFile(filename='',mode='wb',fileobj=buf,compresslevel=3,mtime=0) as f:f.write(dump(doc).encode())
 for r in doc['results']:
  u=r['url'];o=official[u];assert (r['source'],r['blob'],r['environment'])==(o['source'],o['blob'],o['environment']);assert u not in indexByURL
  indexByURL[u]=(k,r,mapping)
def fmt(row,maps):
 if row is None:return None,{}
 ns,collisions=index([(s['name'],s['status']) for s in row['subtests']],maps);assert not collisions,(row['url'],collisions)
 passes=sum(x[0]=='Pass' for x in ns.values());full=bool(ns) and row['status']=='complete' and row['harnessStatus']=='OK' and len(ns)==passes
 return [row['status'],row['harnessStatus'],full,len(ns),passes],ns
with gzip.open(out/'official-pages.jsonl.gz','wt',compresslevel=4) as pages,gzip.open(out/'reference-names.jsonl.gz','wt',compresslevel=4) as names:
 for u,o in official.items():
  rec=indexByURL.get(u);status='observed' if rec else 'awaiting this reference' if o['eligible'] else 'explicitly withheld';state[o['scope']][status]+=1
  k,r,maps=rec or (None,None,{});pg,ns=fmt(r,maps)
  print(dump([u,o['source'],o['blob'],o['environment'],o['scope'],o['guard'],status,o['withheld'],k,pg]),file=pages)
  if not pg:continue
  sc=state[o['scope']];sc.update(fullyPassing=int(pg[2]),named=pg[3],namedPass=pg[4]);sc['page_'+pg[0]]+=1
  for (name,i),(result,original) in ns.items():print(dump([u,name,i,result,original]),file=names)
save('coverage.json',dict(stage=stage,official=len(official),selected=len(eligible),observed=len(indexByURL),completeShards=len(required),scope=state,totals=G(d/'plans/summary.json')))
save('process-events.json',nativeEvents);save('receipt-provenance.json',receipts);save('server-port-maps.json',configs);save('reference-identity.json',ident);save('live-cdp-version.json',G(d/'live-cdp-version.json'))
# Each frozen candidate cohort is reported separately; duplicated measurements of a URL retain their original identity.
if stage in ('history','priority','final'):
 headPages={};headNames=collections.defaultdict(dict);groups=collections.defaultdict(set)
 for cohort,u,src,blob,match,*vals in stream(base/'pages.jsonl.gz'):
  if stage=='history' and cohort!='loss':continue
  headPages[cohort,u]=vals[0];groups[cohort].add(u)
 for cohort,u,n,i,*vals in stream(base/'names.jsonl.gz'):
  if (cohort,u) in headPages and vals[0] is not None:headNames[cohort,u][n,i]=vals[0]
 hist=G(base/'historical-292.json');historical=[]
 grouped=collections.defaultdict(collections.Counter)
 def paired(hpg,hmap,r,mapping,stats,g,u,nf,pf):
  cpg,cm=fmt(r,mapping)
  print(dump([g,u,hpg,cpg]),file=pf)
  if cpg is None:stats['missingChromePages']+=1
  else:stats.update(paired=1,lpFull=int(hpg[2]),chromeFull=int(cpg[2]))
  for n,i in sorted(set(hmap)|set(cm)):
   h=hmap.get((n,i));c=cm.get((n,i));print(dump([g,u,n,i,h,c]),file=nf)
   if cpg is not None:
    hp=h is not None and h[0]=='Pass';cp=c is not None and c[0]=='Pass'
    stats.update(lpPass=int(hp),chromePass=int(cp),lpOnly=int(hp and not cp),chromeOnly=int(cp and not hp),both=int(cp and hp),chromeNameAbsent=int(h is not None and c is None),lpNameAbsent=int(c is not None and h is None))
  return cpg,cm
 with gzip.open(out/'fadc-pages.jsonl.gz','wt',compresslevel=3) as pf,gzip.open(out/'fadc-names.jsonl.gz','wt',compresslevel=3) as nf:
  for (g,u),hpg in headPages.items():
   _,r,maps=indexByURL.get(u,(None,None,{}))
   cpg,cm=paired(hpg,headNames[g,u],r,maps,grouped[g],g,u,nf,pf)
   if g=='loss':
    for h in hist:
     if h['url']==u:historical.append(dict(url=u,canonical=h['canonical'],occurrence=h['occurrence'],fadc=h['observations']['candidate']['case'],chrome158=cm.get((h['canonical'],h['occurrence'])),chrome158Page=cpg,withheld=official[u]['withheld'] if cpg is None else []))
 assert len(historical)==292;save('historical-292.json',historical)
 save('fadc-comparison.json',{'candidate':'fadcbb56ee0afbcbbbed5730c12e24c72480ace6','receiptEvidence':'038635c7abfef92daecb7225bf7bc187e98e8ac3','comparison':grouped})
# Track how many latest LP receipts already match this exact Chromium checkpoint.
lpready=[t for t in required if (lp/'raw'/(t['key']+'.json')).is_file()]
save('cbdeb-join-availability.json',dict(lp='cbdeb11931f74c3de4417a25fb9103f1df84d54e',lpBinarySHA256='81d12af223643b8d5c1903fd528a36a5f2ff3040855f998d7bc5e546805b93b2',matchedCompleteShards=len(lpready),matchedPageCount=sum(t['count'] for t in lpready),keys=[t['key'] for t in lpready]))
print(stage,len(indexByURL),'of',len(official),dict(state),flush=True)
