"""Join available complete cbdeb1193 and Chrome 158 receipts; no browser execution."""
from pathlib import Path
import collections,gzip,json,hashlib,sys
p=Path(__file__).resolve().parent;out=Path(sys.argv[1]);cache=p.parents[1];base=cache/'round6-noncss-measure/manifest-runs';lp=base/'census-cbdeb1193/raw'
sys.path.insert(0,str(base/'census-902fe811f'));from wpt_ports import index,config
G=lambda f:json.loads(f.read_bytes())
def put(f,v):print(json.dumps(v,separators=(',',':'),ensure_ascii=True),file=f)
def save(n,v):(out/n).write_text(json.dumps(v,indent=1)+'\n')
meta=G(out/'receipt-provenance.json');st=collections.defaultdict(collections.Counter);parts={};progress=G(out/'coverage.json')['stage']
with gzip.open(out/'cbdeb-pages.jsonl.gz','wt',compresslevel=3) as ps,gzip.open(out/'cbdeb-names.jsonl.gz','wt',compresslevel=3) as ns:
 for k in meta:
  ref=G(p/'raw'/(k+'.json'));cf=lp/(k+'.json');cm,cproof=config(str(p/'raw'/(k+'.log')));assert 'error' not in cproof
  if not cf.is_file():
   for r in ref['results']:
    scope='css' if r['url'].startswith('css/') else 'non-css'
    st[scope]['missingLpPages']+=1;put(ps,[r['url'],None,[r['status'],r['harnessStatus']],k])
   continue
  cand=G(cf);hm,hproof=config(str(lp/(k+'.log')));assert 'error' not in hproof
  assert cand['wptRevision']==ref['wptRevision'] and cand['inventoryPlan']['viewport']==ref['inventoryPlan']['viewport'] and cand['inventoryPlan']['manifestSha256']==ref['inventoryPlan']['manifestSha256'] and cand['jobs']==ref['jobs']==4
  head={r['url']:r for r in cand['results']};assert set(head)=={r['url'] for r in ref['results']}
  parts[k]={'lpReceiptSHA256':hashlib.sha256(cf.read_bytes()).hexdigest(),'lpLogSHA256':hashlib.sha256((lp/(k+'.log')).read_bytes()).hexdigest(),'lpConfigSHA256':hproof['sha256'],'lpPorts':hproof['ports'],'lpRestarts':cand['restarts'],'lpWatchdogKills':cand['watchdogKills']}
  for r in ref['results']:
   u=r['url'];h=head[u];scope='css' if u.startswith('css/') else 'non-css';ct=st[scope];assert (r['blob'],r['source'],r['environment'])==(h['blob'],h['source'],h['environment'])
   view=[]
   for t,m in ((h,hm),(r,cm)):
    a,clash=index([(s['name'],s['status']) for s in t['subtests']],m);assert not clash
    full=bool(a) and t['status']=='complete' and t['harnessStatus']=='OK' and all(v[0]=='Pass' for v in a.values())
    view.append((a,[t['status'],t['harnessStatus'],full]))
   (hm_,hpg),(cm_,cpg)=view
   put(ps,[u,hpg,cpg,k]);ct.update(paired=1,lpFull=int(hpg[2]),chromeFull=int(cpg[2]))
   for name,i in sorted(hm_.keys()|cm_.keys()):
    a=hm_.get((name,i));b=cm_.get((name,i));hp=a is not None and a[0]=='Pass';cp=b is not None and b[0]=='Pass'
    put(ns,[u,name,i,a,b]);ct.update(lpPass=int(hp),chromePass=int(cp),lpOnly=int(hp and not cp),chromeOnly=int(cp and not hp),both=int(hp and cp),missingLpName=int(a is None and b is not None),missingChromeName=int(b is None and a is not None))
save('cbdeb-comparison.json',dict(head='cbdeb11931f74c3de4417a25fb9103f1df84d54e',binarySHA256='81d12af223643b8d5c1903fd528a36a5f2ff3040855f998d7bc5e546805b93b2',reference='Chrome stable 158.0.8127.37',stage=progress,scopes=st));save('cbdeb-inputs.json',parts)
