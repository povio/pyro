from pathlib import Path
import json,collections,hashlib
d=Path(__file__).parent
def arm(l):
 doc=json.loads((d/(l+'.json')).read_text());out={};pages={}
 for r in doc['results']:
  seen=collections.Counter()
  for s in r['subtests']:
   seen[s['name']]+=1;out[(r['url'],s['name'],seen[s['name']])]=s['status']
  pages[r['url']]={'status':r['status'],'harness':r.get('harnessStatus'),'pass':sum(s['status']=='Pass' for s in r['subtests']),'n':len(r['subtests'])}
 return out,pages
states={l:arm(l) for l in ['parent','candidate','repeat','chrome158']}
keys=set.union(*(set(a) for a,p in states.values()));rows=[dict(url=k[0],name=k[1],occurrence=k[2],**{l:a.get(k) for l,(a,p) in states.items()}) for k in sorted(keys)]
assert states['candidate']==states['repeat']
(d/'outcomes.json').write_text(json.dumps(rows,indent=2));pages={l:p for l,(a,p) in states.items()};(d/'pages.json').write_text(json.dumps(pages,indent=2));print(json.dumps(pages));print(collections.Counter((r['parent'],r['candidate'],r['chrome158']) for r in rows))
