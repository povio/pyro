import json,pathlib
d=pathlib.Path(__file__).resolve().parent
def read(name):return json.loads((d/name).read_text())
labels=['parent','candidate','chrome']
runs=[read(k+'.json') for k in labels]
def names(run):
 assert all(p['status']=='complete' for p in run['results'])
 rows={(p['url'],t['name']):t['status'] for p in run['results'] for t in p['subtests']}
 assert len(rows)==6 and len(run['results'])==2
 return rows
n=list(map(names,runs));assert n[0].keys()==n[1].keys()==n[2].keys()
direct=[read('geometry-direct-'+k+'.json')['results'] for k in labels]
fields=[]
for stage,r in direct[0].items():
 for obj,vs in r.items():
  for f in (range(len(vs)) if isinstance(vs,list) else vs):
   vals=[x[stage][obj][f] for x in direct]
   fields.append(dict(stage=stage,object=obj,field=f,**dict(zip(labels,vals))))
out={'wpt':[dict(url=k[0],name=k[1],**dict(zip(labels,[x[k] for x in n]))) for k in n[0]],'fields':fields,'lostParentMatches':[x for x in fields if x['parent']==x['chrome']!=x['candidate']], 'candidateDifferences':[x for x in fields if x['candidate']!=x['chrome']]}
(d/'comparison.json').write_text(json.dumps(out,indent=2)+'\n')
