"""Export all observed names, including literal names and dynamic port origins."""
from pathlib import Path
import collections,gzip,hashlib,json,shutil,subprocess
n=Path(__file__).resolve().parent
D=n.parent
ref=D.parent/'round6-reference-158/census'
out=n/'evidence'
out.mkdir(exist_ok=True)
g=lambda p:json.loads(p.read_bytes())
def write(f,v): (out/f).write_text(json.dumps(v,indent=2)+'\n')
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
selection=set(g(n/'selection.json')); assert len(selection)==85
pages={}; names={}; inputs={}; configs={}
import sys
sys.path.insert(0,str(D/'manifest-runs/census-902fe811f'))
from wpt_ports import config,canonical_name
def add(lab,f,urls):
 raw=g(f)
 assert len(raw['results'])==len({r['url'] for r in raw['results']})
 assert raw['wptRevision']=='a421f2fea5eb7f91e0db399e44b0fb0779ee1603' and raw['jobs'] in (2,4) and raw['http2'] and raw['testdriver'] and raw['manifestInventory'] and not raw['reuseContext']
 inputs[lab+'/'+f.stem]={'receipt':sha(f),'jobs':raw['jobs'],'restarts':raw['restarts'],'watchdogKills':raw['watchdogKills']}
 mapping,settings=config(str(f.with_suffix('.log'))); assert 'error' not in settings
 port=settings['ports']['https'][0]; configs[lab+'/'+f.stem]={'sha256':settings['sha256'],'httpsPort':port}
 for row in raw['results']:
  u=row['url']
  if u not in urls:continue
  counts=collections.Counter()
  for t in row['subtests']:
   orig=t['name']; canon=canonical_name(orig,mapping)
   if u.endswith('/protocol.https.html'):
    canon=canon.replace('https://web-platform.test:%s'+str(port)+'/', 'https://web-platform.test:%s8443/')
   counts[canon]+=1
   key=(u,canon,counts[canon]); col=names.setdefault(key,{})
   assert lab not in col;col[lab]=[t['status'],orig]
  passes=sum(t['status']=='Pass' for t in row['subtests'])
  total=len(row['subtests'])
  val=[row['status'],row['harnessStatus'],bool(total and total==passes and row['status']=='complete' and row['harnessStatus']=='OK'),passes,total,row['blob'],row['environment']]
  col=pages.setdefault(u,{});assert lab not in col; col[lab]=val
parser=D/'template-combined-8e7144b'
nav=D/'protocol-handlers'
stages={'focus':[parser/'base.json',parser/'head.json'],'guard':[parser/'guard-base.json',parser/'guard-head.json'],'navigator':[nav/'before.json',nav/'after.json']}
for cohort,(base,single) in stages.items():
 urls=set(g(n/(cohort+'.urls.json')))
 expected_jobs=4 if cohort=='navigator' else 2
 for lab,f in [('before',base),('single',single),('after',n/(cohort+'.json'))]:
  raw=g(f)
  assert raw['jobs']==expected_jobs and set(raw['plan'])==urls
  add(lab,f,urls)
for task in g(ref/'plans/tasks.json'):
 key=task['key']; urls=selection & set(g(ref/'selections'/(key+'.json')))
 if urls:add('chrome',ref/'raw'/(key+'.json'),urls)
cols=('before','single','after','chrome')
assert all(set(pages[u])==set(cols) and len({tuple(pages[u][l][5:]) for l in cols})==1 for u in selection)
write('pages.json',pages);write('inputs.json',inputs);write('configs.json',configs)
with gzip.open(out/'names.jsonl.gz','wt') as f:
 for key,val in sorted(names.items()): print(json.dumps([*key,*[val.get(l) for l in cols]],separators=(',',':')),file=f)
def passset(l):return {k for k,v in names.items() if l in v and v[l][0]=='Pass'}
sets={l:passset(l) for l in cols}; a,t,b,c=(sets[l] for l in cols)
stats={l:dict(pages=len(selection),full=sum(v[l][2] for v in pages.values()),passes=len(sets[l])) for l in cols}
stats.update(singleToStackGain=len(b-t),singleToStackLoss=len(t-b),gained=len(b-a),lost=len(a-b),chromeOnly=len(c-b),lpOnly=len(b-c),gainedChrome=len((b-a)&c))
literal={l:{(u,row[l][1],i) for (u,n,i),row in names.items() if l in row and row[l][0]=='Pass'} for l in cols}
stats['singleStackPageChanges']=[u for u in sorted(selection) if pages[u]['single']!=pages[u]['after']];stats['literalChromeOnly']=len(literal['chrome']-literal['after']);stats['literalLpOnly']=len(literal['after']-literal['chrome'])
write('summary.json',stats);print(stats)
write('runtime.json',{'base':'8e7144b7e7de68be3b5b345138abe2c1dcc6d232','candidate':subprocess.check_output(['git','-C',str(n/'src'),'rev-parse','HEAD'],text=True).strip(),'baseBinary':sha(D/'merged-aa0268/integration-52d8b7c/candidate'),'candidateBinary':sha(n/'candidate'),'componentRuntime':{'focusAndGuard':{'commit':'054ffa4c43fa84bdd609b0a02cac2ccb1bb04760','binary':sha(Path(g(parser/'head.json')['bin']))},'navigator':{'commit':'b58b00b07f20042205406ecd76123efcc997d744','binary':sha(nav/'candidate')}},'jsc':subprocess.check_output(['sh','scripts/jsc/cache-key.sh'],cwd=n/'src',text=True).strip(),'wpt':g(n/'navigator.json')['wptRevision'],'chrome':'Google Chrome Stable 158.0.8127.37','referencePublication':'ee24e07f6b15b399d6e8e2157c763809341b6e44'})
for f in ('report.py','run.sh','selection.json','focus.urls.json','guard.urls.json','navigator.urls.json'):shutil.copyfile(n/f,out/f)
