from pathlib import Path
import json,hashlib,gzip,sys,collections
p=Path(__file__).resolve().parent
c=p.parents[1]
ref=c/'round6-reference-158/census'
sys.path.insert(0,str(ref))
from port_receipt import snapshot_config
from wpt_ports import index
def read(f):return json.loads(f.read_bytes())
def sha(f):return hashlib.sha256(f.read_bytes()).hexdigest()
def write(f,x): (p/f).write_text(json.dumps(x,indent=2)+'\n')
sections={k:read(p/(k+'.urls')) for k in ('edit','css','guards')}
urls={u for a in sections.values() for u in a}
rows={};times={};sources={};proof=p/'ports'; proof.mkdir(exist_ok=True)
def ingest(label,f,selected,mapping,original):
 d=read(f); assert d['wptRevision']=='a421f2fea5eb7f91e0db399e44b0fb0779ee1603'
 receipt=sha(f)
 for r in d['results']:
  u=r['url']
  if u not in selected:continue
  k=label,u;assert k not in rows
  ix,collision=index([(s['name'],s['status']) for s in r['subtests']],mapping);assert not collision
  rows[k]=(r,ix);sources.setdefault(label,{})[u]=dict(file=original,sha256=receipt)
for label in ('base','head','chrome'):
 for section,selected in sections.items():
  if label=='chrome' and section!='css':continue
  key=label+'-'+section
  f=p/(key+'.json');log=p/(key+'.log')
  mapping,_=snapshot_config(log,proof/(key+'.json'))
  ingest(label,f,selected,mapping,f.name)
  times[key]={k:read(f).get(k) for k in ['wallMs','browserCpuS','runnerCpuS','serverCpuS','watchdogKills','restarts']}
ci=read(p/'chrome-index.json')
ledger={r['key']:r['receiptSHA256'] for r in map(json.loads,(ref/'raw/completed.jsonl').read_text().splitlines())}
for shard in set(ci.values()):
 f=ref/'raw'/(shard+'.json');assert sha(f)==ledger[shard]
 mapping,_=snapshot_config(f.with_suffix('.log'),ref/'verified-ports/chrome'/(shard+'.json'))
 ingest('chrome',f,{u for u,k in ci.items() if k==shard},mapping,'census/raw/'+f.name)
names=[];pages=[];summary={}
for section,selected in sections.items():
 old=len(names)
 for u in selected:
  three=[rows[l,u] for l in ('base','head','chrome')]
  assert len({r.get('blob') for r,_ in three})==1
  pages.append([u,*[[r['status'],r['harnessStatus'],r['ms']] for r,_ in three]])
  for n,i in sorted(set.union(*(set(ix) for _,ix in three))):
   names.append([u,n,i,*[ix.get((n,i),(None,))[0] for _,ix in three]])
 def stats(a):
  return dict(urls=len({r[0] for r in a}),union=len(a),reported=[sum(r[i]!=None for r in a) for i in (3,4,5)],passes=[sum(r[i]=='Pass' for r in a) for i in (3,4,5)],gained=sum(r[3]!='Pass' and r[4]=='Pass' for r in a),lost=sum(r[3]=='Pass' and r[4]!='Pass' for r in a),gainedChromePass=sum(r[3]!='Pass' and r[4]==r[5]=='Pass' for r in a),lostChromePass=sum(r[3]==r[5]=='Pass' and r[4]!='Pass' for r in a),headOnly=sum(r[4]=='Pass' and r[5]!='Pass' for r in a),chromeOnly=sum(r[5]=='Pass' and r[4]!='Pass' for r in a))
 summary[section]=stats(names[old:])
summary['all']=stats(names)
for k,a in [('names',names),('pages',pages)]:(p/(k+'.jsonl.gz')).write_bytes(gzip.compress((''.join(json.dumps(v)+'\n' for v in a)).encode(),mtime=0))
write('summary.json',summary);write('timings.json',times);write('receipts.json',sources)
print(json.dumps(summary))
