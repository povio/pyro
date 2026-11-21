import collections,gzip,json,hashlib,sys,shutil
from pathlib import Path
p=Path(__file__).resolve().parent;c=p.parents[1];d=c/'round6-noncss-measure'
sys.path.insert(0,str(d/'manifest-runs/census-902fe811f'))
from wpt_ports import config,index
ref=c/'round6-reference-158/census'; us=set(json.loads((p/'inheritance-selection.json').read_text()));seen={};pg={};sources={}
out=p/'inherit-evidence';out.mkdir(exist_ok=True)
def save(n,x):(out/n).write_text(json.dumps(x,indent=1)+'\n')
def add(fn,lab):
 raw=json.loads(fn.read_bytes());mp,info=config(str(fn.with_suffix('.log')));assert 'error' not in info
 assert raw['wptRevision']=='a421f2fea5eb7f91e0db399e44b0fb0779ee1603'
 sources[lab+'/'+fn.name]=dict(sha256=hashlib.sha256(fn.read_bytes()).hexdigest(),portMap=info['ports'],jobs=raw['jobs'],restarts=raw['restarts'],watchdogKills=raw['watchdogKills'])
 for row in raw['results']:
  u=row['url']
  if u not in us:continue
  key=u,lab;assert key not in seen
  a,cl=index([(z['name'],z['status']) for z in row['subtests']],mp);assert not cl;seen[key]=a
  passes=sum(t[0]=='Pass' for t in a.values())
  pg[key]=[row['status'],row['harnessStatus'],bool(a) and passes==len(a) and row['status']=='complete' and row['harnessStatus']=='OK',passes,len(a)]
for l,f in [('base','inherit-base'),('candidate','inherit-head')]:add(p/(f+'.json'),l)
for f in (ref/'selections').glob('*.json'):
 if set(json.loads(f.read_bytes()))&us:add(ref/'raw'/f.name,'chrome')
labs=['base','candidate','chrome'];ct=collections.Counter()
with gzip.open(out/'names.jsonl.gz','wt') as nf,gzip.open(out/'pages.jsonl.gz','wt') as pf:
 for u in sorted(us):
  vv=[seen[u,l] for l in labs];print(json.dumps([u,*[pg[u,l] for l in labs]]),file=pf)
  for l in labs:ct[l+'/full']+=pg[u,l][2];ct[l+'/'+pg[u,l][0]]+=1
  for name,occ in sorted(set.union(*(set(t) for t in vv))):
   obs=[t.get((name,occ)) for t in vv];print(json.dumps([u,name,occ,*obs]),file=nf)
   aa,bb,cc=[bool(t and t[0]=='Pass') for t in obs]
   for l,z in zip(labs,(aa,bb,cc)):ct[l+'/pass']+=z
   ct['gains']+=bb and not aa;ct['losses']+=aa and not bb;ct['chromeOnly']+=cc and not bb;ct['candidateOnly']+=bb and not cc
save('summary.json',ct);save('receipts.json',sources)
for f in ['inheritance-selection.json','blank.cjs','initial-blank.json','inherit-blank.json','chr-blank.json','inherit-report.py','inheritance.sh']:shutil.copy2(p/f,out/f)
print(ct)
