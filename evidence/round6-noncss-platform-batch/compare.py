"""Compare disjoint exact selections, retaining original names and canonical duplicate occurrences."""
from pathlib import Path
import collections,gzip,json,sqlite3,zlib,sys
p=Path(__file__).resolve().parent;d=p.parent
sys.path.insert(0,str(d/'manifest-runs/census-902fe811f'))
from wpt_ports import endpoint_map,config,index
db=sqlite3.connect(f'file:{d}/manifest-runs/scores.sqlite?mode=ro',uri=True)
cohort=sys.argv[1]
defs={'main':([('platform-control','control'),('platform-extra-control','extra-control')],[('platform-final','final'),('platform-extra-final','extra-final')]),
      'historical':([('platform-loss-control','loss-control')],[('platform-loss-final','loss-final')])}
if len(sys.argv)>2:
 defs[cohort]=(defs[cohort][0],[(sys.argv[2],sys.argv[3])]+([('platform-extra-final','extra-final')] if cohort=='main' else []))
def read(sources):
 acc={}
 for label,logname in sources:
  doc=json.loads((d/'reconciled'/(label+'.json')).read_bytes());portmap,proof=config(str(p/(logname+'.log')))
  assert doc['wptRevision']=='a421f2fea5eb7f91e0db399e44b0fb0779ee1603' and doc['jobs']==4 and 'error' not in proof
  for r in doc['results']:
   u=r['url'];assert u not in acc;acc[u]=r,portmap
 return acc
old,new=map(read,defs[cohort]);assert old.keys()==new.keys()
stats=[collections.Counter() for _ in range(3)];delta=collections.Counter();loss=[];areas=collections.defaultdict(collections.Counter);refs=collections.Counter()
def fmt(r,m):
 n,collision=index([(s['name'],s['status']) for s in r['subtests']],m);assert not collision
 return n,bool(n) and r['status']=='complete' and r['harnessStatus']=='OK' and all(v=='Pass' for v,raw in n.values())
def dump(v):return json.dumps(v,separators=(',',':'))
with gzip.open(p/(cohort+'-names.jsonl.gz'),'wt',compresslevel=5) as ns,gzip.open(p/(cohort+'-pages.jsonl.gz'),'wt') as ps:
 for u in sorted(old):
  blob,path=db.execute("select raw,path from result where label='chromeWpt' and url=?",(u,)).fetchone()
  stable=json.loads(zlib.decompress(blob));cm,proof=endpoint_map(path,u);assert 'error' not in proof
  rows=[old[u],new[u],(stable,cm)];assert len({r['blob'] for r,_ in rows})==1
  maps=[];page=[]
  for i,(r,m) in enumerate(rows):
   n,full=fmt(r,m);maps.append(n);page.append([r['status'],r['harnessStatus'],full]);stats[i][r['status']]+=1
   stats[i].update(namedPass=sum(v=='Pass' for v,_ in n.values()),full=int(full))
  print(dump([u,*page]),file=ps)
  for name,num in sorted(set().union(*maps)):
   vals=[nm.get((name,num)) for nm in maps];statuses=[c[0] if c else None for c in vals]
   a,b,c=statuses
   if b=='Pass' and a!='Pass':delta['gained']+=1;areas[u.split('/')[0]]['gained']+=1
   if a=='Pass' and b!='Pass':delta['lost']+=1;areas[u.split('/')[0]]['lost']+=1;loss.append([u,name,num,*statuses])
   if b=='Pass' and c!='Pass':refs['onlyLP']+=1
   if c=='Pass' and b!='Pass':refs['onlyChrome']+=1
   delta['missing']+=a is not None and b is None;delta['added']+=a is None and b is not None
   print(dump([u,name,num,*vals]),file=ns)
summary=dict(urls=len(old),counts=stats,delta=delta,localChrome=refs,byArea=areas,losses=loss)
(p/(cohort+'-summary.json')).write_text(json.dumps(summary,indent=2))
print(cohort,len(old),stats,delta,refs)
