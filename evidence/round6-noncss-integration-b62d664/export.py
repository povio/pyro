"""Export new paired LP guards and separately identified prior and comparator observations."""
import collections,gzip,hashlib,json,shutil,subprocess,sys
from pathlib import Path
p=Path(__file__).resolve().parent;out=p/'verified';d=p.parents[1];c=p.parents[2];ref=c/'round6-reference-158/census';olds=p.parent/'integration-52d8b7c/verified';nav=d/'template-navigator-combined/evidence';doc=d/'template-inert-documents'
sys.path.insert(0,str(d/'manifest-runs/census-902fe811f'));from wpt_ports import config,canonical_name
G=lambda f:json.loads(f.read_bytes());sha=lambda f:hashlib.sha256(f.read_bytes()).hexdigest()
def rows(f):
 with gzip.open(f,'rt') as stream:yield from map(json.loads,stream)
def put(f,x):f.write_text(json.dumps(x,indent=1)+'\n')
def emit(f,x):print(json.dumps(x,separators=(',',':')),file=f)
out.mkdir(exist_ok=True);tasks=G(p/'tasks.json');us={u:t['cohort'] for t in tasks for u in G(p/'selections'/(t['key']+'.json'))}
official={r['url']:r for r in rows(ref/'plans/official-inventory.jsonl.gz') if r['url'] in us};assert len(official)==len(us)==458
cols=('base','head','prior-local','chrome-local','studio-lp','chrome-studio')
pg={u:{} for u in us};named=collections.defaultdict(dict);prov={};maps={};ident=G(ref/'reference-identity.json')
assert sha(ref/'runner/bench/wpt/run-no-native-retry.mjs')==ident['noNativeReplayRunnerSHA256']
def raw(label,f,allow,tid=None):
 raw=G(f);assert allow=={r['url'] for r in raw['results']};assert raw['wptRevision']==ident['wpt'] and raw['inventoryPlan']['manifestSha256']==ident['manifestSha256']
 assert raw['testdriver'] and raw['http2'] and raw['server']=='wptserve' and not raw['reuseContext']
 if label in ('base','head'):
  t=tid;assert raw['jobs']==t['jobs'] and Path(raw['bin']).resolve()==p/label
  ledger={v['key']:v for l in (f.parent/'completed.jsonl').read_text().splitlines() if (v:=json.loads(l))}
  assert sha(f)==ledger[t['key']]['sha256']
 log=f.with_suffix('.log')
 if label=='prior-local' and f.stem=='head':log=f.parent/'run-head.log'
 mapping,cfg=config(str(log));assert 'error' not in cfg
 maps[cfg['sha256']]=cfg['ports'];port=cfg['ports']['https'][0]
 prov[label+'/'+f.stem]=dict(sha256=sha(f),logSHA256=sha(log),configSHA256=cfg['sha256'],jobs=raw['jobs'],restarts=raw['restarts'],watchdogKills=raw['watchdogKills'])
 for r in raw['results']:
  u=r['url'];o=official[u];assert [r[k] for k in ['source','blob','environment']]==[o[k] for k in ['source','blob','environment']]
  cnt=collections.Counter();npass=0
  for a in r['subtests']:
   orig=a['name'];n=canonical_name(orig,mapping)
   if u.endswith('/protocol.https.html'):n=n.replace('https://web-platform.test:%s'+str(port)+'/', 'https://web-platform.test:%s8443/')
   cnt[n]+=1;k=(u,n,cnt[n]);assert label not in named[k];named[k][label]=[a['status'],orig];npass+=a['status']=='Pass'
  total=len(r['subtests']);assert label not in pg[u];pg[u][label]=[r['status'],r['harnessStatus'],bool(total and npass==total and r['status']=='complete' and r['harnessStatus']=='OK'),npass,total]
for t in tasks:
 urls=set(G(p/'selections'/(t['key']+'.json')))
 for label in ['base','head']:raw(label,p/('raw-'+label)/(t['key']+'.json'),urls,t)
for co,filename in [('focus','head.json'),('guard','guard-head.json')]:
 packed=subprocess.check_output(['git','show','44867401f:evidence/round6-inert-document-markers/'+filename+'.gz'])
 published=json.loads(gzip.decompress(packed));cached=G(doc/filename)
 assert published['results']==cached['results'] and published['plan']==cached['plan']
 raw('prior-local',doc/filename,{u for u in us if us[u]==co})
# Validate cached immutable publications by comparing Git blob IDs for each input consumed.
def checkpub(folder,commit,sub,names):
 for n in names:
  f=folder/n;h=subprocess.check_output(['git','hash-object',str(f)],text=True).strip()
  dest=commit+':evidence/'+sub+'/'+n
  assert subprocess.check_output(['git','rev-parse',dest],text=True).strip()==h,dest
checkpub(olds,'144976264','round6-noncss-integration-52d8b7c',['pages.jsonl.gz','named.jsonl.gz'])
checkpub(nav,'05599687f','round6-template-navigator-combined',['pages.json','names.jsonl.gz'])
for u,ps in G(nav/'pages.json').items():
 if u not in us:continue
 pg[u]['chrome-local']=ps['chrome'][:5]
 if us[u]=='navigator':pg[u]['prior-local']=ps['after'][:5]
for u,n,i,x,y,z,cr in rows(nav/'names.jsonl.gz'):
 if u in us:
  if cr:named[u,n,i]['chrome-local']=cr
  if z and us[u]=='navigator':named[u,n,i]['prior-local']=z
for u,k,co,scope,dims,b,h,cr in rows(olds/'pages.jsonl.gz'):
 if u not in us or us[u] not in ('production','css'):continue
 if us[u]=='production':pg[u]['prior-local']=h[:5]
 if cr:pg[u]['chrome-local']=cr[:5]
for u,n,i,b,h,cr in rows(olds/'named.jsonl.gz'):
 if u not in us or us[u] not in ('production','css'):continue
 if us[u]=='production' and h:named[u,n,i]['prior-local']=h
 if cr:named[u,n,i]['chrome-local']=cr
# Studio rows report numeric harness status and literal names without local port attribution.
ex=ref/'export-final';checkpub(ex,'ee24e07f6','round6-chrome158',['studio-css-pages.jsonl.gz','studio-css-names.jsonl.gz'])
added_local=set()
for u,source,blob,env,co,st,lo,k,x,y in rows(ex/'studio-css-pages.jsonl.gz'):
 if u not in us:continue
 assert us[u]=='css' and [source,blob,env]==[official[u][k] for k in ['source','blob','environment']]
 if st:pg[u]['chrome-studio']=st[:3]+[st[4],st[3]]
 # Additional local CSS overlap may lie outside the prior integration guard.
 if lo:
  if 'chrome-local' not in pg[u]:added_local.add(u)
  v=lo[:3]+[lo[4],lo[3]]
  assert pg[u].get('chrome-local',v)==v;pg[u]['chrome-local']=v
for u,n,i,st,lo in rows(ex/'studio-css-names.jsonl.gz'):
 if u not in us:continue
 if st:named[u,n,i]['chrome-studio']=[st,n]
 if lo and u in added_local:named[u,n,i]['chrome-local']=[lo,n]
studio=p/'src/docs/validation/studio-css-e4ca26'
for stem in ('main','supplement'):
 f=studio/('outcomes-flexcrossbound-'+stem+'-candidate.json.gz')
 o=json.load(gzip.open(f));prov['studio-lp/'+stem]={'sha256':sha(f)}
 for u,v in o.items():
  if u not in us:continue
  s,hs,tests=v;cnt=collections.Counter();npass=0
  for n,status in tests:
   cnt[n]+=1;named[u,n,cnt[n]]['studio-lp']=[status,n];npass+=status=='Pass'
  pg[u]['studio-lp']=[s,hs,bool(tests and s=='complete' and hs in (0,'OK') and npass==len(tests)),npass,len(tests)]
with gzip.open(out/'pages.jsonl.gz','wt') as fp:
 for u,co in sorted(us.items()):
  o=official[u];emit(fp,[u,co,{k:o[k] for k in ['source','blob','environment','eligible','withheld']},*[pg[u].get(l) for l in cols]])
with gzip.open(out/'names.jsonl.gz','wt') as fp:
 for key,v in sorted(named.items()):
  if v:emit(fp,[*key,*[v.get(l) for l in cols]])
put(out/'provenance.json',prov);put(out/'ports.json',maps);put(out/'columns.json',cols);put(out/'selected.json',us)
for n in ['tasks.json','holds.json','production-selection.json','plan.py','run.py','audit.py','export.py']:shutil.copy2(p/n,out/n)
shutil.copy2(ref/'reference-identity.json',out/'chrome-identity.json');shutil.copy2(doc/'controls-chrome.json',out/'controls-chrome-original.json');shutil.copy2(doc/'controls-head.json',out/'controls-prior.json');shutil.copy2(doc/'probe.cjs',out/'controls.cjs')
