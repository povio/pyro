import json,collections,pathlib,sys
d=pathlib.Path(__file__).parent
for suite in ['core','opposite']:
 f=d if suite=='core' else d/suite
 arms={n:json.loads((f/('scripts-'+v+'.json')).read_bytes())['rows'] for n,v in [('parent','base'),('candidate',sys.argv[1]),('signed','chrome')]}
 scores={};loss=[]
 for i,(x,y,z) in enumerate(zip(*arms.values())):
  for st,v in z.items():
   if st in ['before','target','selection','name','cmd','shape','css','reverse']:continue
   for k,e in (v.items() if isinstance(v,dict) else [(st,v)]):
    get=lambda r:r[st][k] if isinstance(v,dict) else r[st]
    if get(y)==e:scores.setdefault(st,collections.Counter())[k]+=1
    elif get(x)==e:loss.append(dict(fixture={k:z[k] for k in ['cmd','shape','css','reverse','name'] if k in z},stage=st,field=k,arms={n:get(rs[i]) for n,rs in arms.items()}))
 print(suite,len(arms['candidate']),scores['after'],collections.Counter((l['stage'],l['field']) for l in loss))
 for k,obj in [('scores',scores),('losses',loss)]:(f/(sys.argv[1]+'-'+k+'.json')).write_text(json.dumps(obj,indent=2)+'\n')
