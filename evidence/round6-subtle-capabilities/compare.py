"""Emit occurrence-preserving, full outer matched comparisons from WPT receipts."""
import collections,gzip,json,sqlite3,sys,zlib
from pathlib import Path
m=Path(__file__).resolve().parent;d=m.parent
sys.path.insert(0,str(d/'manifest-runs/census-902fe811f'))
from wpt_ports import endpoint_map,config,index
out=Path(sys.argv[1]);out.mkdir(exist_ok=True)
plan=json.loads(Path(sys.argv[2]).read_bytes())
allowed=set(json.loads((d/plan['selected']).read_bytes())) if plan.get('selected') else None
def read(parts):
 allrows={}
 for label,log in parts:
  f=d/label[1:] if label.startswith('@') else d/'reconciled'/(label+'.json')
  report=json.loads(f.read_bytes());mapping,proof=config(str(d/log))
  assert 'error' not in proof and report['wptRevision']=='a421f2fea5eb7f91e0db399e44b0fb0779ee1603' and (report.get('jobs')==4 or report.get('partial') and label in plan.get('preservedPartials',[]))
  assert not report.get('restarts',0) and not report.get('watchdogKills',0)
  for row in report['results']:
   u=row['url']
   if allowed is not None and u not in allowed:continue
   assert u not in allrows
   allrows[u]=(row,mapping)
 return allrows
db=sqlite3.connect(f'file:{d}/manifest-runs/scores.sqlite?mode=ro',uri=True)
old=read(plan['control']);new=read(plan['candidate']);assert old.keys()==new.keys()
reference={}
if plan.get('reference')=='chrome158':
 ref=d.parent/'round6-reference-158/census'
 for record in map(json.loads,(ref/'raw/completed.jsonl').read_text().splitlines()):
  key=record['key'];urls=json.loads((ref/'selections'/(key+'.json')).read_bytes())
  if not old.keys() & set(urls):continue
  run=json.loads((ref/'raw'/(key+'.json')).read_bytes());assert run['label']=='chrome158'
  cm,proof=config(str(ref/'raw'/(key+'.log')));assert 'error' not in proof
  for row in run['results']:
   if row['url'] in old:reference[row['url']]=(row,cm)
 if plan.get('supplementalChrome'):
  for u,value in read(plan['supplementalChrome']).items():
   if u in old:reference[u]=value
 if not plan.get('allowMissingReference'):
  assert old.keys()==reference.keys(),(len(old),len(reference))

stats=[collections.Counter() for _ in range(3)]
diff=collections.Counter();areas=collections.defaultdict(collections.Counter);losses=[];perurl={}
def line(file,values):print(json.dumps(values,separators=(',',':')),file=file)
with gzip.open(out/'pages.jsonl.gz','wt') as pf,gzip.open(out/'names.jsonl.gz','wt',compresslevel=5) as nf:
 for u in sorted(old):
  if plan.get('reference')=='chrome158':cr,cm=reference.get(u,(None,{}))
  else:
   raw,f=db.execute("select raw,path from result where label='chromeWpt' and url=?",(u,)).fetchone();cr=json.loads(zlib.decompress(raw));cm,proof=endpoint_map(f,u);assert 'error' not in proof
  records=[old[u],new[u],(cr,cm)];assert len({r['blob'] for r,_ in records if r is not None})==1
  ns=[];page=[]
  for (r,mp),sc in zip(records,stats):
   if r is None:
    ns.append({});page.append(None);sc['unobserved']+=1;continue
   n,c=index([(x['name'],x['status']) for x in r['subtests']],mp);assert not c
   ns.append(n);passed=sum(status=='Pass' for status,name in n.values())
   full=bool(n) and r['status']=='complete' and r['harnessStatus']=='OK' and passed==len(n)
   page.append([r['status'],r['harnessStatus'],full]);sc.update(namedPass=passed,full=int(full),reported=len(n));sc[r['status']]+=1
  line(pf,[u,*page]);pc=collections.Counter()
  for k in sorted(set().union(*ns)):
   cols=[n.get(k) for n in ns];a,b,c=[x[0] if x else None for x in cols];line(nf,[u,*k,*cols])
   pc.update(gained=b=='Pass' and a!='Pass',lost=a=='Pass' and b!='Pass',added=a is None and b is not None,missing=b is None and a is not None,chromeOnly=c=='Pass' and b!='Pass',lpOnly=cr is not None and b=='Pass' and c!='Pass')
   if a=='Pass' and b!='Pass':losses.append([u,*k,*cols])
  perurl[u]=pc;diff.update(pc);areas[u.split('/')[0]].update(pc)
s=dict(urls=len(old),reference=plan.get('reference','historical Chrome 157.0.8103.80'),counts=stats,delta=diff,areas=areas,losses=losses,perURL=perurl)
(out/'summary.json').write_text(json.dumps(s,indent=2));print(len(old),stats,diff)
