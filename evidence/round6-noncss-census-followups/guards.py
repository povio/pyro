from pathlib import Path
from collections import Counter
import sys,zlib,gzip,sqlite3,json
p=Path(__file__).resolve().parent;root=p.parent
labels=sys.argv[1:-1];key=sys.argv[-1]
def rows(path):
 d=json.loads(path.read_bytes());assert d['wptRevision']=='a421f2fea5eb7f91e0db399e44b0fb0779ee1603' and d['jobs']==4
 return {r['url']:r for r in d['results']}
docs=[rows(root/'reconciled'/(v+'.json')) for v in labels]
urls=set(docs[0]);assert all(set(x)==urls for x in docs)
db=sqlite3.connect(f'file:{root}/manifest-runs/scores.sqlite?mode=ro',uri=True)
docs.append({u:json.loads(zlib.decompress(db.execute("select raw from result where label='chromeWpt' and url=?",(u,)).fetchone()[0])) for u in urls})
counts=[Counter() for _ in docs];diff=Counter();net=Counter();losses=[]
def index(row):
 c=Counter();a={}
 for s in row['subtests']:
  n=s['name'];c[n]+=1;a[n,c[n]]=s['status']
 return a
def dump(x):return json.dumps(x,ensure_ascii=True,separators=(',',':'))
with gzip.open(p/(key+'-names.jsonl.gz'),'wt') as nf,gzip.open(p/(key+'-pages.jsonl.gz'),'wt') as pf:
 for u in sorted(urls):
  rs=[d[u] for d in docs]; assert len({r['blob'] for r in rs})==1
  maps=[index(r) for r in rs];page=[]
  for r,m,c in zip(rs,maps,counts):
   full=bool(m) and r['status']=='complete' and r.get('harnessStatus')=='OK' and all(v=='Pass' for v in m.values())
   c.update(namedPass=sum(v=='Pass' for v in m.values()),full=int(full));c[r['status']]+=1
   page.append([r['status'],r.get('harnessStatus'),full])
  print(dump([u,*page]),file=pf)
  for n,i in sorted(set().union(*maps)):
   values=[m.get((n,i)) for m in maps]
   if values[0]!='Pass' and values[1]=='Pass':diff['gained']+=1;net[u]+=1
   if values[0]=='Pass' and values[1]!='Pass':diff['lost']+=1;net[u]-=1;losses.append([u,n,i,*values])
   diff['missing']+=values[0] is not None and values[1] is None
   diff['added']+=values[0] is None and values[1] is not None
   print(dump([u,n,i,*values]),file=nf)
summary=dict(urls=len(urls),columns=[*labels,'Chrome WPT 157.0.8103.80'],counts=counts,delta=diff,losses=losses,netByUrl=net)
(p/(key+'-summary.json')).write_text(json.dumps(summary,indent=2))
print(key,counts,diff)
