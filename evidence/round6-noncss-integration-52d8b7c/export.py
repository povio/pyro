"""Build matched three-way evidence from frozen receipts. Takes an output directory."""
import collections, gzip, hashlib, json, os, shutil, sys
from pathlib import Path
cache=Path(os.environ.get('LP_CACHE',Path.home()/'Library/Caches/lightningpanda'))
measure=cache/'round6-noncss-measure'
p=measure/'merged-aa0268/integration-52d8b7c'
ref=cache/'round6-reference-158/census'
base=measure/'manifest-runs/census-cbdeb1193/raw'
sys.path.insert(0,str(measure/'manifest-runs/census-902fe811f'))
from wpt_ports import index,config
out=Path(sys.argv[1]);out.mkdir(parents=True,exist_ok=True)
G=lambda f:json.loads(f.read_bytes())
sha=lambda f:hashlib.sha256(f.read_bytes()).hexdigest()
def save(n,v):(out/n).write_text(json.dumps(v,indent=1,ensure_ascii=True)+'\n')
def emit(f,d):f.write(json.dumps(d,separators=(',',':'),ensure_ascii=True)+'\n')
tasks=G(p/'tasks.json')
sel=lambda root,k:G(root/'selections'/(k+'.json'))
selected={}
for t in tasks:
 k=t['key'];f=p/'selections'/(k+'.json');us=G(f)
 assert sha(f)==t['sha256'] and len(us)==t['count']
 for u in us:
  assert u not in selected;co=t.get('supplement') or ('additional-noncss' if k.startswith('wave2-') else 'wasm-metadata' if k.startswith('wasm-metadata') else 'upstream-css' if k.startswith('upstream-css') else 'priority')
  selected[u]=(k,co,t['scope'])
assert sum(v[1]=='priority' for v in selected.values())==4041
ref_tasks=G(ref/'plans/tasks.json')
receipts={};views={};ports={}
def read(label,root,key,planned,source=None):
 ledger=root/'completed.jsonl'
 records=[r for l in ledger.read_text().splitlines() if (r:=json.loads(l))['key']==key] if ledger.exists() else []
 if source is None and not records:return
 f=(root/(key+'.json')) if source is None else source;log=f.with_suffix('.log')
 s=sha(f)
 if source is None:assert len(records)==1 and records[0]['receiptSHA256']==s,(label,key)
 raw=G(f);rows=raw['results']
 if label in ('base','head'):
  assert raw['label']=='lp' and Path(raw['bin']).resolve()==p/('base' if label=='base' else 'candidate')
 assert set(planned)=={v['url'] for v in rows} and len(planned)==len(rows)
 assert raw['jobs']==4 and raw['testdriver'] and raw['http2'] and raw['server']=='wptserve' and not raw['reuseContext'] and raw['manifestInventory']
 assert raw['wptRevision']==identity['wpt'] and raw['inventoryPlan']['manifestSha256']==identity['manifestSha256']
 assert raw['inventoryPlan']['viewport']==dict(width=800,height=600,deviceScaleFactor=1)
 mp,pr=config(str(log));assert 'error' not in pr,pr
 ports[pr['sha256']]=pr['ports']
 receipts[label+'/'+key]=dict(sha256=s,logSHA256=sha(log),serverConfigSHA256=pr['sha256'],reported=len(rows),restarts=raw['restarts'],watchdogKills=raw['watchdogKills'],source='signed-census' if label=='chrome' and source is None else 'same-profile-standalone' if source is not None else 'measured-LP')
 for row in rows:
  u=row['url']
  if u not in selected or (source is not None and label in views.get(u,([],{}))[1]):continue
  table,collisions=index([(t['name'],t['status']) for t in row['subtests']],mp);assert not collisions,(u,collisions)
  passed=sum(v[0]=='Pass' for v in table.values())
  full=bool(table) and passed==len(table) and row['status']=='complete' and row['harnessStatus']=='OK'
  dims=[row[a] for a in ('source','blob','environment','protocol')]
  assert dims[:3]==[official[u][x] for x in ('source','blob','environment')]
  prev,obs=views.setdefault(u,(dims,{}));assert prev==dims and label not in obs
  obs[label]=([row['status'],row['harnessStatus'],full,passed,len(table),key],table)
official={r['url']:r for line in gzip.open(ref/'plans/official-inventory.jsonl.gz','rt') if (r:=json.loads(line))['url'] in selected}
assert set(official)==set(selected)
for u,(k,co,scope) in selected.items():
 if co in ('priority','extra-feature','additional-noncss'):assert official[u]['eligible']
 if co in ('wasm-storage-supplement','wasm-metadata'):assert not official[u]['eligible']
identity=G(ref/'reference-identity.json')
assert sha(ref/'runner/bench/wpt/run-no-native-retry.mjs')==identity['noNativeReplayRunnerSHA256']
assert sha(cache/'round6-inventory/MANIFEST-pinned.json')==identity['manifestSha256']
for t in tasks:
 k=t['key'];us=sel(p,k)
 read('base',p/'raw-base',k,us);read('head',p/'raw-head',k,us)
for t in ref_tasks:
 k=t['key'];us=sel(ref,k)
 if selected.keys() & set(us):
  assert sha(ref/'selections'/(k+'.json'))==t['sha256']
  read('chrome',ref/'raw',k,us)
for tag,f in [('idb',measure/'indexeddb-bindings/chrome158.json'),('input',measure/'input-target-cbdeb/chrome158.json'),('crypto',measure/'merged-aa0268/chacha/chrome158-wpt.json'),('metadata',cache/'round6-wasm-reflection/chrome158.json'),('storage',measure/'merged-aa0268/wasm-storage-alignment/chrome158.json'),('public-key',measure/'merged-aa0268/public-key/chrome158-wpt.json'),('crypto-support',measure/'merged-aa0268/crypto-support/chrome158-wpt.json'),('offscreen',measure/'merged-aa0268/offscreen/chrome158-wpt.json'),('editing',measure/'input-grapheme-85150/editing-chrome158.json')]:
 raw=G(f);assert raw['label'] in ('chrome158','editing-chrome158') and Path(raw['bin']).resolve()==ref/'launch-158'
 read('chrome',f.parent,'standalone-'+tag,raw['plan'],source=f)
save('coverage-reasons.json',{u:{'frozenEligible':o['eligible'],'originalWithheld':o['withheld']} for u,o in official.items()})
hist=G(ref/'export-priority/historical-292.json')
causes=G(measure/'manifest-runs/census-902fe811f/historical-losses/rows.json')
cause={(r['url'],r['canonical'],r['occurrence']):r for r in causes}
hstats=collections.defaultdict(lambda:collections.defaultdict(collections.Counter))
for r in hist:
 u=r['url'];key=r['canonical'],r['occurrence'];v=views.get(u,([],{}))[1];old=cause[u,*key];r.update(originalCause=old['classification'],originalDetail=old['detail'])
 for label in ['base','head','chrome']:
  page,names=v.get(label,(None,{}));val=names.get(key);r[label+'Page']=page;r[label]=val
  if label=='chrome' and r['chrome158Page']:assert list(val) == r['chrome158'] if val else r['chrome158'] is None
  status='unobserved-page' if page is None else ('missing-name' if val is None else val[0])
  hstats['all'][label][status]+=1
  if u.startswith('referrer-policy/'):hstats['referrer'][label][status]+=1
assert len(hist)==292 and sum(hstats['referrer']['head'].values())==157
save('historical-292.json',hist);save('historical-summary.json',hstats)
stats=collections.defaultdict(collections.Counter)
with gzip.open(out/'pages.jsonl.gz','wt') as pf,gzip.open(out/'named.jsonl.gz','wt',compresslevel=4) as nf:
 for u,(task,cohort,scope) in sorted(selected.items()):
  dims,obs=views.get(u,([],{}));ps=[];tables=[];ct=stats[cohort+'/'+scope];ct['planned']+=1
  for label in ('base','head','chrome'):
   pg,names=obs.get(label,(None,{}));ps.append(pg);tables.append(names)
   ct[label+'/missingPage' if not pg else label+'/page/'+pg[0]]+=1
   if pg:ct[label+'/fullPages']+=pg[2];ct[label+'/passes']+=pg[3]
  emit(pf,[u,task,cohort,scope,dims,*ps])
  for i,j,tag in ((0,1,'base-to-head'),(2,1,'chrome-to-head')):
   if ps[i] and ps[j]:
    ct[tag+'/pairedPages']+=1;ct[tag+'/gainedFullPages']+=ps[j][2] and not ps[i][2];ct[tag+'/lostFullPages']+=ps[i][2] and not ps[j][2]
  for k in sorted(set().union(*tables)):
   vals=[a.get(k) for a in tables]
   emit(nf,[u,*k,*vals])
   for i,j,tag in ((0,1,'base-to-head'),(2,1,'chrome-to-head')):
    if not ps[i] or not ps[j]:continue
    a,b=vals[i],vals[j];ap=a is not None and a[0]=='Pass';bp=b is not None and b[0]=='Pass'
    ct[tag+'/gainedPass']+=bp and not ap;ct[tag+'/lostPass']+=ap and not bp;ct[tag+'/commonPass']+=ap and bp
    ct[tag+'/addedName']+=b is not None and a is None;ct[tag+'/missingName']+=a is not None and b is None
save('summary.json',stats);save('receipt-provenance.json',receipts);save('server-ports.json',ports)
save('tasks.json',tasks);save('chrome-identity.json',identity);save('selected-urls.json',selected)
print(json.dumps(stats))
